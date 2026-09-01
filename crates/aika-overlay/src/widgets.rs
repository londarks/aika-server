//! Finding out what the client's user interface actually does, by asking it.
//!
//! The client keeps its full RTTI, so every widget class is named in the
//! binary and its vtable can be found without a debugger. Reading them gave a
//! hierarchy rooted at `SControl` with 34 virtual methods, and exactly three
//! slots that *every* derived class overrides: 0, 2 and 25. Nought is the
//! destructor, which is true of any class with virtual methods and says
//! nothing.
//!
//! Slot 25 is the interesting one. In `SControl` it assembles to `C2 14 00` —
//! `ret 20` and nothing else, an empty body that pops five stack arguments.
//! An empty base implementation that every subclass replaces is what a
//! framework's "the derived class fills this in" hook looks like, and there
//! are only two things it can plausibly be: drawing, or input.
//!
//! Static reading could not separate those two, so counting did. Against a
//! running client the answer was exact multiples of the frame count —
//! `SControl` once a frame, `SPanel` eight times, `SButton` and `SText` four,
//! `SEditableText` twice — and the panel count rose the moment a window was
//! opened. Slot 25 is the draw, and it runs once per visible widget per frame.
//!
//! That is what makes the interface ours to replace: a widget is drawn by this
//! call and by nothing else, so returning without calling the original removes
//! it from the screen, one class or one instance at a time, with no patching of
//! the client's own code.
//!
//! `F9` toggles it, so the proof needs no rebuild: press it and the interface
//! goes; press it again and it is back, because nothing was destroyed — a
//! frame's worth of drawing was simply not done.
//!
//! # Telling one window from another
//!
//! Hiding a whole class is not what anyone wants; hiding *that* bar is. So
//! `F10` writes down every widget drawn in the next frame — its class, its
//! address and its first bytes. Two snapshots, one before opening a window and
//! one after, name the objects that window is made of, and the fields that
//! move when it moves are its position. It is the method this project already
//! uses on the data files, pointed at memory instead of at a file.

use crate::{inspect, rules};
use crate::{log, patch_vtable};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

/// The slot every widget class overrides, found by diffing the vtables.
const DRAW_CANDIDATE: usize = 25;

/// Widget vtables, as offsets from the image base.
///
/// Offsets rather than addresses because the executable carries relocations
/// and can be loaded anywhere; the base is read at runtime. They were taken
/// from the RTTI in `AIKA.exe` and are true for that build alone — a client
/// patch moves every one of them, which is why they are listed in one place.
const VTABLES: [(&str, usize); 9] = [
    ("SControl", 0x31_F780),
    ("SPanel", 0x31_FF80),
    ("SButton", 0x31_E12C),
    ("SText", 0x31_F320),
    ("SListBox", 0x31_FEE8),
    ("SScrollBar", 0x31_EEC4),
    ("SProgressBar", 0x31_DEB0),
    ("SEditableText", 0x31_E1CC),
    ("SMessageBox", 0x31_DD6C),
];

/// `this` in `ECX` and five arguments on the stack, which is what `ret 20`
/// says the base implementation expects. Naming the arguments would be a
/// guess; counting them is not.
type Slot25 = unsafe extern "thiscall" fn(*mut c_void, u32, u32, u32, u32, u32);

/// What each vtable had in slot 25 before we replaced it.
static ORIGINALS: [AtomicUsize; VTABLES.len()] =
    [const { AtomicUsize::new(0) }; VTABLES.len()];
/// How many times each class's slot 25 has run since the last report.
static CALLS: [AtomicU64; VTABLES.len()] = [const { AtomicU64::new(0) }; VTABLES.len()];

/// Addresses of the patched vtables, so a call can be traced back to the class
/// it came through. A COM-style object still points at its own vtable after a
/// single entry inside it is replaced, so the pointer identifies the class.
static PATCHED: [AtomicUsize; VTABLES.len()] =
    [const { AtomicUsize::new(0) }; VTABLES.len()];

/// Whether the client's own interface is being drawn. Toggled with `F9`.
static SHOW_ORIGINAL_UI: AtomicBool = AtomicBool::new(true);

/// Set by `F10`, cleared after one frame: write down every widget drawn.
///
/// One frame rather than a duration, because a frame *is* the unit — it holds
/// exactly what is on screen once, with no repeats to wade through. At four
/// thousand calls a second any window longer than that produces a log nobody
/// can read.
static SNAPSHOT: AtomicBool = AtomicBool::new(false);

/// How much of a widget to write down.
///
/// The client's own code reads fields at `+0x80` and `+0x90` while drawing an
/// `SPanel`, so whatever decides where a window sits lives at least that far
/// in. Taking a round 0xC0 covers it with room to spare, and the object is
/// certainly that big or its own code would be reading past itself.
const DUMP_BYTES: usize = 0xC0;

/// Asks for a snapshot on the next frame.
pub fn request_snapshot() {
    SNAPSHOT.store(true, Ordering::Relaxed);
    log("[widgets] --- snapshot: every widget drawn in the next frame ---");
}

/// Ends the snapshot. Called once a frame has gone by.
pub fn end_snapshot() -> bool {
    SNAPSHOT.swap(false, Ordering::Relaxed)
}

/// Writes down one widget: which class, which object, and its first bytes.
///
/// The bytes are shown three ways because none alone is enough. As hex to see
/// the layout, as floats because a position on screen is a float and reads as
/// nonsense in any other base, and as integers because a flag or an id does
/// not. Whatever identifies a window is one of the three, and comparing two
/// snapshots — before and after opening a window — is what says which.
unsafe fn describe(class: &str, this: *mut c_void, a: u32, b: u32, c: u32, d: u32, e: u32) {
    let words = std::slice::from_raw_parts(this as *const u32, DUMP_BYTES / 4);

    log(&format!(
        "[snap] {class} this=0x{:08X} args=0x{a:X},0x{b:X},0x{c:X},0x{d:X},0x{e:X}",
        this as usize
    ));

    for row in 0..words.len() / 8 {
        let at = row * 8;
        let slice = &words[at..at + 8];
        let hex: Vec<String> = slice.iter().map(|w| format!("{w:08X}")).collect();
        // A float only earns printing when it could be a coordinate: the screen
        // is at most a few thousand pixels across, so anything huge or
        // vanishingly small is some other kind of number wearing a float's bits.
        let floats: Vec<String> = slice
            .iter()
            .map(|w| {
                let f = f32::from_bits(*w);
                if f != 0.0 && f.abs() > 0.01 && f.abs() < 10_000.0 {
                    format!("{f:.1}")
                } else {
                    "-".to_string()
                }
            })
            .collect();
        log(&format!(
            "[snap]   +{:03X} {} | {}",
            at * 4,
            hex.join(" "),
            floats.join(" ")
        ));
    }
}

/// Turns the client's interface on or off. Returns what it became.
pub fn toggle_original_ui() -> bool {
    let showing = !SHOW_ORIGINAL_UI.load(Ordering::Relaxed);
    SHOW_ORIGINAL_UI.store(showing, Ordering::Relaxed);
    log(if showing {
        "[widgets] the client's interface is drawing again"
    } else {
        "[widgets] the client's interface is hidden; the screen is the overlay's"
    });
    showing
}

/// Replaces slot 25 in every listed vtable. Safe to call more than once: a
/// vtable already patched is skipped, so the original can never be lost.
pub unsafe fn install() {
    let base = GetModuleHandleA(std::ptr::null()) as usize;
    if base == 0 {
        log("[widgets] could not find the image base");
        return;
    }
    log(&format!("[widgets] image base 0x{base:08X}"));

    for (i, (name, rva)) in VTABLES.iter().enumerate() {
        if PATCHED[i].load(Ordering::Acquire) != 0 {
            continue;
        }
        let vtable = base + rva;

        // The vtable is data, not an object, so there is no pointer to
        // dereference first — `patch_vtable` takes the object, hence the
        // address of the address.
        let holder = &vtable as *const usize as *mut c_void;
        let original = patch_vtable(holder, DRAW_CANDIDATE, probe as *const c_void);
        if original == 0 {
            log(&format!("[widgets] {name}: could not patch"));
            continue;
        }

        ORIGINALS[i].store(original, Ordering::Release);
        PATCHED[i].store(vtable, Ordering::Release);
        log(&format!(
            "[widgets] {name}: slot {DRAW_CANDIDATE} at 0x{vtable:08X} -> was 0x{original:08X}"
        ));
    }
}

/// Stands in for slot 25 on every patched class at once.
///
/// One function rather than one per class because the object says which class
/// it is: its first word is the vtable it was patched in, which is enough to
/// find the original to call.
unsafe extern "thiscall" fn probe(
    this: *mut c_void,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    e: u32,
) {
    let vtable = *(this as *const usize);

    for i in 0..VTABLES.len() {
        if PATCHED[i].load(Ordering::Relaxed) != vtable {
            continue;
        }
        CALLS[i].fetch_add(1, Ordering::Relaxed);

        if SNAPSHOT.load(Ordering::Relaxed) {
            describe(VTABLES[i].0, this, a, b, c, d, e);
        }

        // Not calling the original is the whole mechanism. The widget is not
        // destroyed, disabled or moved; this frame simply does not draw it, and
        // the next frame will if the switch has flipped back.
        if !SHOW_ORIGINAL_UI.load(Ordering::Relaxed) {
            return;
        }

        // Recorded before the rules are applied, so the inspector shows the
        // interface as the client laid it out rather than as we bent it.
        if inspect::is_on() {
            inspect::record(VTABLES[i].0, this as *const u8);
        }

        let rule = rules::matching(VTABLES[i].0, this as *const u8);
        if rule.is_some_and(|r| r.hide) {
            return;
        }

        // A widget being dragged follows the mouse instead of its rule, so what
        // is on screen while dragging is what will be saved.
        let dragging = crate::dragging_shift(this as usize);

        // A move is put in place for the length of the draw and taken out
        // again. The client owns this field: leaving it shifted would have it
        // read a position it did not write, add the offset once more on the
        // next frame, and walk the window off the screen.
        let moved = dragging
            .or_else(|| rule.filter(|r| r.shift != (0, 0)).map(|r| r.shift))
            .map(|by| rules::shift(this as *mut u8, by));

        let original = ORIGINALS[i].load(Ordering::Relaxed);
        if original != 0 {
            let original: Slot25 = std::mem::transmute(original);
            original(this, a, b, c, d, e);
        }

        if let Some(previous) = moved {
            rules::restore(this as *mut u8, previous);
        }
        return;
    }

    // A vtable we did not patch cannot reach here, so arriving means the
    // object's first word is not what was assumed. Returning without calling
    // anything is the only safe answer.
    log("[widgets] a call arrived through an unknown vtable");
}

/// Writes what was counted and starts again.
///
/// Called from the frame hook every so often rather than every frame: the
/// question is the ratio between calls and frames, and one line per frame
/// would cost more than it tells.
pub fn report(frames: u64) {
    let mut line = format!("[widgets] {frames} frames |");
    let mut any = false;
    for (i, (name, _)) in VTABLES.iter().enumerate() {
        let calls = CALLS[i].swap(0, Ordering::Relaxed);
        if calls == 0 {
            continue;
        }
        any = true;
        line.push_str(&format!(" {name}={calls}"));
    }
    if any {
        log(&line);
    } else {
        log(&format!(
            "[widgets] {frames} frames | slot {DRAW_CANDIDATE} never ran"
        ));
    }
}
