//! A `d3d9.dll` that sits between the game client and the real one.
//!
//! The client imports exactly one function from `d3d9.dll`, `Direct3DCreate9`,
//! which was read out of its import table rather than assumed. That is the
//! whole surface this library has to present: Windows searches the directory
//! the executable lives in before `System32` for a DLL that is not a KnownDLL,
//! so dropping this file next to `AIKA.exe` is enough to be loaded in place of
//! the system one, and forwarding that single call is enough for the game to
//! run exactly as it did.
//!
//! Being in the middle is the point. From `Direct3DCreate9` the whole chain is
//! reachable: the object it returns creates the device, and the device is what
//! draws every frame. Each step replaces one entry of a COM vtable and calls
//! the original, so nothing is reimplemented and the client keeps rendering
//! its own characters, animation and terrain.
//!
//! # Why a vtable entry and not a detour
//!
//! A COM object is a pointer to a table of function pointers. Swapping one
//! entry needs no disassembler, no trampoline and no instruction rewriting: it
//! is a pointer write behind `VirtualProtect`. The classic five-byte detour
//! exists for functions that are not reached through a table, which is not the
//! case here.
//!
//! # What this build does
//!
//! Loads the real library, forwards the call, hooks `CreateDevice` and then
//! `Present`, and fills a small rectangle in the corner. The rectangle is the
//! proof that the overlay is inside the client's render loop and has the
//! device in its hands; everything drawn later goes in the same place.
//!
//! From there [`widgets`] reaches the client's own interface, which turned out
//! to be drawn through a single virtual call. `F9` stops that call from
//! running, and the interface disappears without anything being patched or
//! destroyed.
//!
//! Built for `i686-pc-windows-msvc`, because the client is a 32-bit process.

#![cfg(windows)]

mod inspect;
mod rules;
mod widgets;

use std::ffi::{c_void, CString};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{FARPROC, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameA, GetModuleHandleA, GetProcAddress, LoadLibraryA,
};
use windows_sys::Win32::System::Memory::{
    VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryA;
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

// ---------------------------------------------------------------------------
// Vtable indices
// ---------------------------------------------------------------------------

/// `IDirect3D9::CreateDevice`, counted from `QueryInterface` at zero. The three
/// `IUnknown` methods come first in every COM vtable, then the sixteen the
/// interface declares in `d3d9.h`, in declaration order.
const IDIRECT3D9_CREATE_DEVICE: usize = 16;

/// `IDirect3DDevice9::Present`. `Reset` is the entry before it, and hooking
/// that one is what a later version needs so device-owned resources can be
/// released before the driver throws them away.
const IDIRECT3DDEVICE9_PRESENT: usize = 17;

/// `IDirect3DDevice9::Clear`. Used rather than any drawing call because it
/// needs no vertex buffer, no shader and no state block: given a rectangle it
/// fills it, which is all a proof of life has to do.
const IDIRECT3DDEVICE9_CLEAR: usize = 43;

/// `D3DCLEAR_TARGET`: clear the colour buffer and leave depth and stencil be.
const D3DCLEAR_TARGET: u32 = 0x1;

// ---------------------------------------------------------------------------
// Function types
// ---------------------------------------------------------------------------

type Direct3DCreate9Fn = unsafe extern "system" fn(u32) -> *mut c_void;

type CreateDeviceFn = unsafe extern "system" fn(
    *mut c_void, // this
    u32,         // adapter
    u32,         // device type
    *mut c_void, // focus window
    u32,         // behaviour flags
    *mut c_void, // presentation parameters
    *mut *mut c_void,
) -> i32;

type PresentFn = unsafe extern "system" fn(
    *mut c_void, // this
    *const c_void,
    *const c_void,
    *mut c_void,
    *const c_void,
) -> i32;

type ClearFn = unsafe extern "system" fn(
    *mut c_void, // this
    u32,         // rectangle count
    *const D3dRect,
    u32, // flags
    u32, // colour
    f32, // depth
    u32, // stencil
) -> i32;

/// `D3DRECT`: the region `Clear` fills. Four signed longs, in that order.
#[repr(C)]
struct D3dRect {
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The originals, kept as addresses because a function pointer is not something
/// an atomic can hold directly. Zero means "not hooked yet".
static REAL_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static REAL_PRESENT: AtomicUsize = AtomicUsize::new(0);
static REAL_CLEAR: AtomicUsize = AtomicUsize::new(0);

static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Whether the device vtable has already been patched. `CreateDevice` runs
/// again on a resolution change, and hooking a second time would store our own
/// function as the original and call it forever.
static DEVICE_HOOKED: AtomicBool = AtomicBool::new(false);

/// The window the device draws into, kept from `CreateDevice`. The mouse is
/// reported in screen coordinates and the interface is laid out in the
/// window's, so one is needed to get to the other.
static GAME_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// The back buffer's size, which is what the interface's coordinates are in.
static SCREEN: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// Appends a line to `aika-overlay.log`, beside this library.
///
/// A file rather than a console: the client is a GUI process with no stdout to
/// print to, and its own `AikaClient.log` records nothing but that it started.
///
/// The path is built from where this DLL was loaded from rather than left
/// relative, because a relative path lands in the working directory, and a
/// game started from a launcher does not necessarily have the one you expect.
/// A test whose only output cannot be found reads exactly like a test that
/// failed.
///
/// Failures are swallowed — a log that cannot be written is not a reason to
/// take the game down with it.
fn log(message: &str) {
    let _ = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())?;
        writeln!(file, "{message}")
    })();
}

/// Where the rules file sits: beside the library, for the same reason the log
/// does.
fn rules_path() -> std::path::PathBuf {
    let mut path = log_path();
    path.set_file_name("aika-overlay.rules");
    path
}

fn log_path() -> std::path::PathBuf {
    static ONCE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let fallback = || std::path::PathBuf::from("aika-overlay.log");
        unsafe {
            let module = GetModuleHandleA(c"d3d9.dll".as_ptr() as *const u8);
            if module.is_null() {
                return fallback();
            }
            let mut buffer = [0u8; 520];
            let written =
                GetModuleFileNameA(module, buffer.as_mut_ptr(), buffer.len() as u32) as usize;
            if written == 0 {
                return fallback();
            }
            let mut path = std::path::PathBuf::from(
                String::from_utf8_lossy(&buffer[..written]).into_owned(),
            );
            path.set_file_name("aika-overlay.log");
            path
        }
    })
    .clone()
}

// ---------------------------------------------------------------------------
// Loading the real library
// ---------------------------------------------------------------------------

/// Opens the real `d3d9.dll` from the system directory.
///
/// The path is spelled out rather than left to the search order, which is the
/// whole reason this file gets loaded at all: asking for `d3d9.dll` by name
/// from inside the game's own directory would find this library again and
/// deadlock the loader against itself.
unsafe fn real_d3d9() -> HMODULE {
    static HANDLE: AtomicUsize = AtomicUsize::new(0);

    let cached = HANDLE.load(Ordering::Acquire);
    if cached != 0 {
        return cached as HMODULE;
    }

    let mut buffer = [0u8; 260];
    let written = GetSystemDirectoryA(buffer.as_mut_ptr(), buffer.len() as u32) as usize;
    let mut path = String::from_utf8_lossy(&buffer[..written]).into_owned();
    path.push_str("\\d3d9.dll");

    let c_path = CString::new(path.clone()).expect("a path cannot hold a NUL");
    let module = LoadLibraryA(c_path.as_ptr() as *const u8);
    if module.is_null() {
        log(&format!("[overlay] could not load {path}"));
        return std::ptr::null_mut();
    }

    log(&format!("[overlay] real library loaded from {path}"));
    HANDLE.store(module as usize, Ordering::Release);
    module
}

unsafe fn export(module: HMODULE, name: &str) -> FARPROC {
    let c_name = CString::new(name).expect("a symbol cannot hold a NUL");
    GetProcAddress(module, c_name.as_ptr() as *const u8)
}

// ---------------------------------------------------------------------------
// Vtable patching
// ---------------------------------------------------------------------------

/// Replaces one entry of a COM object's vtable and returns what was there.
///
/// `object` is a pointer to a pointer to the table, which is what a COM
/// interface pointer is. The page holding the table is read-only, hence the
/// `VirtualProtect` around the write and the second call putting the old
/// protection back — leaving a driver's table writable is a change nobody
/// asked for.
unsafe fn patch_vtable(object: *mut c_void, index: usize, replacement: *const c_void) -> usize {
    let vtable = *(object as *mut *mut usize);
    let entry = vtable.add(index);

    let mut previous: PAGE_PROTECTION_FLAGS = 0;
    if VirtualProtect(
        entry as *mut c_void,
        std::mem::size_of::<usize>(),
        PAGE_EXECUTE_READWRITE,
        &mut previous,
    ) == 0
    {
        log("[overlay] could not make the vtable writable");
        return 0;
    }

    let original = *entry;
    *entry = replacement as usize;

    let mut ignored: PAGE_PROTECTION_FLAGS = 0;
    VirtualProtect(
        entry as *mut c_void,
        std::mem::size_of::<usize>(),
        previous,
        &mut ignored,
    );

    original
}

// ---------------------------------------------------------------------------
// The hooks
// ---------------------------------------------------------------------------

/// Stands in for `IDirect3DDevice9::Present`, which the client calls once per
/// frame. Anything drawn before the original runs is on the screen the player
/// sees; anything after it is on the next frame, or on none.
unsafe extern "system" fn present(
    device: *mut c_void,
    source: *const c_void,
    destination: *const c_void,
    window: *mut c_void,
    dirty: *const c_void,
) -> i32 {
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);

    // The first frame is worth a line; a line per frame would write gigabytes.
    if frame == 0 {
        log("[overlay] first frame: the overlay is inside the render loop");
        // Only now, with the client fully up and a device in hand, is it worth
        // patching the interface: whatever the count says then is about a
        // running game rather than a loading screen.
        widgets::install();
        rules::reload(&rules_path());
    }

    // Roughly every few seconds at any sane frame rate. What matters is the
    // ratio of widget calls to frames, and that needs both counted over the
    // same window.
    if frame > 0 && frame % 300 == 0 {
        widgets::report(300);
    }

    // A snapshot lasts exactly one frame, and this is the end of it.
    widgets::end_snapshot();

    poll_hotkeys();

    draw_proof_of_life(device);
    draw_inspector(device);

    // Only now is the frame's list of widgets finished with. `Present` runs at
    // the *end* of a frame, after every widget has already drawn itself into
    // that list, so clearing it any earlier throws away the very thing the
    // inspector is about to draw. It did exactly that, and the symptom was F8
    // appearing to do nothing at all.
    inspect::begin_frame();

    let original: PresentFn = std::mem::transmute(REAL_PRESENT.load(Ordering::Acquire));
    original(device, source, destination, window, dirty)
}

/// Reads the keys the overlay answers to, once per frame.
///
/// `F9` hides the client's interface, `F10` writes down every widget drawn in
/// the next frame.
///
/// Polled from inside `Present` rather than through a keyboard hook or a
/// window procedure of our own: the frame loop is a thread we are already on,
/// and it runs often enough that a keypress is never missed. It also means the
/// overlay adds nothing to the client's message pump, which is the part most
/// likely to be sensitive to a stranger's code.
///
/// The edge is what matters, not the state: without remembering the previous
/// frame, holding the key down would toggle sixty times a second.
fn poll_hotkeys() {
    const VK_F9: i32 = 0x78;
    static WAS_DOWN: AtomicBool = AtomicBool::new(false);

    let down = unsafe { GetAsyncKeyState(VK_F9) as u16 & 0x8000 != 0 };
    if down && !WAS_DOWN.swap(true, Ordering::Relaxed) {
        widgets::toggle_original_ui();
    } else if !down {
        WAS_DOWN.store(false, Ordering::Relaxed);
    }

    const VK_F7: i32 = 0x76;
    static GRAB_WAS_DOWN: AtomicBool = AtomicBool::new(false);

    let down = unsafe { GetAsyncKeyState(VK_F7) as u16 & 0x8000 != 0 };
    if down && !GRAB_WAS_DOWN.swap(true, Ordering::Relaxed) && inspect::is_on() {
        let screen = SCREEN.load(Ordering::Relaxed);
        let screen = ((screen >> 16) as i32, (screen & 0xFFFF) as i32);
        if let Some(rule) = inspect::grab_or_drop(cursor_in_window(), screen) {
            rules::append_move(&rules_path(), &rule);
            rules::reload(&rules_path());
        }
    } else if !down {
        GRAB_WAS_DOWN.store(false, Ordering::Relaxed);
    }

    const VK_F8: i32 = 0x77;
    static INSPECT_WAS_DOWN: AtomicBool = AtomicBool::new(false);

    let down = unsafe { GetAsyncKeyState(VK_F8) as u16 & 0x8000 != 0 };
    if down && !INSPECT_WAS_DOWN.swap(true, Ordering::Relaxed) {
        inspect::toggle();
    } else if !down {
        INSPECT_WAS_DOWN.store(false, Ordering::Relaxed);
    }

    const VK_F11: i32 = 0x7A;
    static RELOAD_WAS_DOWN: AtomicBool = AtomicBool::new(false);

    let down = unsafe { GetAsyncKeyState(VK_F11) as u16 & 0x8000 != 0 };
    if down && !RELOAD_WAS_DOWN.swap(true, Ordering::Relaxed) {
        rules::reload(&rules_path());
    } else if !down {
        RELOAD_WAS_DOWN.store(false, Ordering::Relaxed);
    }

    const VK_F10: i32 = 0x79;
    static SNAP_WAS_DOWN: AtomicBool = AtomicBool::new(false);

    let down = unsafe { GetAsyncKeyState(VK_F10) as u16 & 0x8000 != 0 };
    if down && !SNAP_WAS_DOWN.swap(true, Ordering::Relaxed) {
        widgets::request_snapshot();
    } else if !down {
        SNAP_WAS_DOWN.store(false, Ordering::Relaxed);
    }
}

/// Draws the inspector: every widget outlined, the one under the mouse filled.
///
/// At the end of the frame, so the boxes land on top of the interface they
/// describe rather than under it.
unsafe fn draw_inspector(device: *mut c_void) {
    if !inspect::is_on() {
        return;
    }
    let clear = REAL_CLEAR.load(Ordering::Acquire);
    if clear == 0 {
        return;
    }

    let screen = SCREEN.load(Ordering::Relaxed);
    let screen = ((screen >> 16) as i32, (screen & 0xFFFF) as i32);
    let picked = cursor_in_window().and_then(|at| inspect::pick(at, screen));
    inspect::report_pick(picked);

    let (strokes, solid) = inspect::outlines(picked.map(|b| b.this));
    let clear: ClearFn = std::mem::transmute(clear);

    let to_rects = |v: Vec<(i32, i32, i32, i32)>| -> Vec<D3dRect> {
        v.into_iter()
            .map(|(x, y, w, h)| D3dRect { x1: x, y1: y, x2: x + w, y2: y + h })
            .collect()
    };

    // The highlight first, so an outline drawn over it still reads.
    let solid = to_rects(solid);
    if !solid.is_empty() {
        clear(device, solid.len() as u32, solid.as_ptr(), D3DCLEAR_TARGET, 0xFF_7C_6A_EF, 1.0, 0);
    }
    let strokes = to_rects(strokes);
    if !strokes.is_empty() {
        clear(device, strokes.len() as u32, strokes.as_ptr(), D3DCLEAR_TARGET, 0xFF_4A_DE_80, 1.0, 0);
    }
}

/// How far a widget being dragged should be shifted this frame, if it is the
/// one being dragged. Lives here because the mouse does.
pub(crate) fn dragging_shift(this: usize) -> Option<(i16, i16)> {
    if !inspect::is_grabbing() {
        return None;
    }
    inspect::drag_shift(this, cursor_in_window())
}

/// The mouse, in the coordinates the interface is laid out in.
fn cursor_in_window() -> Option<(i32, i32)> {
    let window = GAME_WINDOW.load(Ordering::Relaxed);
    if window == 0 {
        return None;
    }
    unsafe {
        let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 {
            return None;
        }
        if ScreenToClient(window as _, &mut point) == 0 {
            return None;
        }
        Some((point.x, point.y))
    }
}

/// Fills a small rectangle in the top-left corner.
///
/// `Clear` is used deliberately: drawing a textured quad needs a vertex buffer,
/// a shader and a saved state block, and getting any of that wrong looks
/// exactly like the hook not working. A filled rectangle either appears or it
/// does not, so what it proves is unambiguous.
unsafe fn draw_proof_of_life(device: *mut c_void) {
    let clear = REAL_CLEAR.load(Ordering::Acquire);
    if clear == 0 {
        return;
    }

    // Alternates so a frozen frame is not mistaken for a running one.
    let colour = if (FRAMES.load(Ordering::Relaxed) / 30) % 2 == 0 {
        0xFF_7C_6A_EF // ARGB
    } else {
        0xFF_4A_DE_80
    };

    let rect = D3dRect { x1: 16, y1: 16, x2: 176, y2: 48 };
    let clear: ClearFn = std::mem::transmute(clear);
    clear(device, 1, &rect, D3DCLEAR_TARGET, colour, 1.0, 0);
}

/// Stands in for `IDirect3D9::CreateDevice`. The device does not exist until
/// this returns, which is why the frame hooks can only be placed from here.
unsafe extern "system" fn create_device(
    this: *mut c_void,
    adapter: u32,
    device_type: u32,
    focus_window: *mut c_void,
    behaviour: u32,
    parameters: *mut c_void,
    returned: *mut *mut c_void,
) -> i32 {
    let original: CreateDeviceFn = std::mem::transmute(REAL_CREATE_DEVICE.load(Ordering::Acquire));
    let result = original(
        this,
        adapter,
        device_type,
        focus_window,
        behaviour,
        parameters,
        returned,
    );

    if result < 0 || returned.is_null() || (*returned).is_null() {
        log(&format!("[overlay] CreateDevice failed: 0x{result:08X}"));
        return result;
    }

    // A resolution change creates a device again. Hooking twice would store our
    // own function as the original and call it forever.
    if DEVICE_HOOKED.swap(true, Ordering::AcqRel) {
        return result;
    }

    GAME_WINDOW.store(focus_window as usize, Ordering::Release);

    // `D3DPRESENT_PARAMETERS` opens with the back buffer's width and height,
    // which is the space the interface's coordinates live in. Taking it from
    // here rather than from the window avoids the borders and the title bar.
    if !parameters.is_null() {
        let dimensions = parameters as *const u32;
        let (width, height) = (*dimensions, *dimensions.add(1));
        if width > 0 && height > 0 {
            SCREEN.store(((width as usize) << 16) | height as usize, Ordering::Release);
            log(&format!("[overlay] back buffer {width} x {height}"));
        }
    }

    let device = *returned;
    REAL_PRESENT.store(
        patch_vtable(device, IDIRECT3DDEVICE9_PRESENT, present as *const c_void),
        Ordering::Release,
    );
    // Kept as a pointer to call, not as a hook: the overlay draws with it.
    REAL_CLEAR.store(
        *(*(device as *mut *mut usize)).add(IDIRECT3DDEVICE9_CLEAR),
        Ordering::Release,
    );

    log("[overlay] device created, Present hooked");
    result
}

// ---------------------------------------------------------------------------
// The export
// ---------------------------------------------------------------------------

/// The one function the client imports.
///
/// `extern "system"` is what makes this `stdcall` on a 32-bit target, which is
/// what `WINAPI` means and what the client's call site expects. A `cdecl`
/// function here would leave the stack unbalanced on every call.
#[no_mangle]
pub unsafe extern "system" fn Direct3DCreate9(sdk_version: u32) -> *mut c_void {
    let module = real_d3d9();
    if module.is_null() {
        return std::ptr::null_mut();
    }

    let Some(create) = export(module, "Direct3DCreate9") else {
        log("[overlay] the real library has no Direct3DCreate9");
        return std::ptr::null_mut();
    };

    let create: Direct3DCreate9Fn = std::mem::transmute(create);
    let d3d9 = create(sdk_version);
    if d3d9.is_null() {
        log("[overlay] the real Direct3DCreate9 returned nothing");
        return d3d9;
    }

    log(&format!("[overlay] Direct3DCreate9(0x{sdk_version:X}) forwarded"));

    REAL_CREATE_DEVICE.store(
        patch_vtable(d3d9, IDIRECT3D9_CREATE_DEVICE, create_device as *const c_void),
        Ordering::Release,
    );

    d3d9
}
