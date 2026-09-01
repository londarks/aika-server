//! Seeing the interface as boxes, so a layout can be changed by looking at it.
//!
//! Reading a snapshot in a log and matching numbers to what is on screen is
//! work, and it is the wrong kind: the information is already spatial, and a
//! list of rectangles is the worst possible way to show a person where things
//! are. So this draws them where they are.
//!
//! Every widget drawn in a frame is collected with its rectangle on screen,
//! and at the end of the frame each one is outlined. The rectangle under the
//! mouse is filled instead, and named in the log, which turns "which window is
//! the 1023 by 63 one" into pointing at it.
//!
//! # Drawn with `Clear`
//!
//! An outline is four thin filled rectangles, and `Clear` fills rectangles —
//! it takes an array of them, so a whole frame of outlines is one call. That
//! matters more than it sounds: text and textures would mean a vertex buffer,
//! a shader and a saved state block, which is a renderer, which is the thing
//! this deliberately is not yet. Boxes need none of it.

use crate::{log, rules};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Whether the inspector is running. Off costs one atomic read per widget.
static ON: AtomicBool = AtomicBool::new(false);

pub fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Turns the inspector on or off. Returns what it became.
pub fn toggle() -> bool {
    let on = !ON.load(Ordering::Relaxed);
    ON.store(on, Ordering::Relaxed);
    if on {
        log("[inspect] on: every widget is outlined, and the one under the mouse is filled");
    } else {
        log("[inspect] off");
        FRAME.lock().unwrap().clear();
    }
    on
}

/// The widget the mouse was last over, so a move is only reported when it
/// changes rather than sixty times a second.
static LAST_PICKED: Mutex<Option<usize>> = Mutex::new(None);

/// Writes down what is under the mouse, when it changes.
///
/// The line is written in the exact shape of a rule, so choosing a window and
/// hiding it is copying a line rather than composing one from a log.
pub fn report_pick(picked: Option<Box>) {
    let mut last = LAST_PICKED.lock().unwrap();
    let now = picked.map(|b| b.this);
    if *last == now {
        return;
    }
    *last = now;

    if let Some(b) = picked {
        log(&format!(
            "[inspect] {} on screen at ({},{}) {}x{}   ->   hide  {}  {}  {}  {}  {}",
            b.class, b.x, b.y, b.width, b.height,
            b.class, b.width, b.height, b.local.0, b.local.1
        ));
    }
}

/// One widget, as it was drawn this frame.
#[derive(Clone, Copy)]
pub struct Box {
    pub class: &'static str,
    /// Where it is on the screen, the whole chain of parents added up.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Its own position, the one the object stores and a rule matches on. Not
    /// the same as `x`/`y` unless the widget sits at the root.
    pub local: (i16, i16),
    pub this: usize,
}

/// The widget being dragged, if any: which one, and how far it has been taken
/// from where the client put it.
///
/// Held here rather than written into the object because a drag is not a
/// decision yet — it becomes one when it is dropped and a rule is written. Up
/// to that point the client's own position is left exactly as it was, so
/// letting go of the idea costs nothing.
static GRABBED: Mutex<Option<(usize, (i32, i32), (i16, i16))>> = Mutex::new(None);

/// Picks up whatever is under the mouse, or puts down what is held.
///
/// Returns the rule to save when something was dropped, so the caller decides
/// what to do with it — this module knows about widgets, not about files.
pub fn grab_or_drop(at: Option<(i32, i32)>, screen: (i32, i32)) -> Option<crate::rules::Rule> {
    let mut grabbed = GRABBED.lock().unwrap();

    if let Some((this, origin, local)) = *grabbed {
        let shift = at.map_or((0, 0), |now| (now.0 - origin.0, now.1 - origin.1));
        *grabbed = None;

        let held = FRAME.lock().unwrap().iter().find(|b| b.this == this).copied();
        let Some(b) = held else {
            log("[inspect] dropped, but the widget is no longer on screen; nothing saved");
            return None;
        };
        if shift == (0, 0) {
            log("[inspect] dropped where it was picked up; nothing to save");
            return None;
        }

        log(&format!(
            "[inspect] dropped {} at ({},{}), moved {},{}",
            b.class, b.x + shift.0, b.y + shift.1, shift.0, shift.1
        ));
        return Some(crate::rules::Rule {
            class: b.class,
            width: b.width as i16,
            height: b.height as i16,
            at: (Some(local.0), Some(local.1)),
            hide: false,
            shift: (shift.0 as i16, shift.1 as i16),
        });
    }

    let at = at?;
    let b = pick(at, screen)?;
    *grabbed = Some((b.this, at, b.local));
    log(&format!(
        "[inspect] holding {} {}x{}; move the mouse and press F7 again to drop it",
        b.class, b.width, b.height
    ));
    None
}

/// How far the held widget should be drawn from where the client put it.
pub fn drag_shift(this: usize, at: Option<(i32, i32)>) -> Option<(i16, i16)> {
    let grabbed = GRABBED.lock().unwrap();
    let (held, origin, _) = (*grabbed)?;
    if held != this {
        return None;
    }
    let now = at?;
    Some(((now.0 - origin.0) as i16, (now.1 - origin.1) as i16))
}

pub fn is_grabbing() -> bool {
    GRABBED.lock().unwrap().is_some()
}

/// What was drawn this frame, refilled from scratch every frame.
///
/// A mutex rather than an atomic anything: this is written from the draw calls
/// and read at the end of the frame, both on the render thread, and the lock is
/// uncontended. Being wrong here would be a crash in somebody's game, and the
/// cost of being right is nothing.
static FRAME: Mutex<Vec<Box>> = Mutex::new(Vec::new());

/// How thick an outline is drawn, in pixels.
const STROKE: i32 = 1;

/// Ignore anything bigger than this fraction of the screen when picking with
/// the mouse. The root panel covers everything, so without this the answer to
/// "what am I pointing at" would always be "the whole screen".
const PICK_MAX_AREA: f32 = 0.6;

pub fn begin_frame() {
    FRAME.lock().unwrap().clear();
}

/// Records a widget, if it has a rectangle worth drawing.
pub unsafe fn record(class: &'static str, this: *const u8) {
    let Some((x, y, width, height)) = rules::absolute_rect(this) else {
        return;
    };
    let position = this.add(rules::POSITION) as *const i16;
    FRAME.lock().unwrap().push(Box {
        class,
        x,
        y,
        width,
        height,
        local: (*position, *position.add(1)),
        this: this as usize,
    });
}

/// The smallest widget containing a point, which is the one a person means when
/// they point at overlapping boxes.
pub fn pick(at: (i32, i32), screen: (i32, i32)) -> Option<Box> {
    let limit = (screen.0 * screen.1) as f32 * PICK_MAX_AREA;
    FRAME
        .lock()
        .unwrap()
        .iter()
        .filter(|b| (b.width * b.height) as f32 <= limit)
        .filter(|b| {
            at.0 >= b.x && at.0 < b.x + b.width && at.1 >= b.y && at.1 < b.y + b.height
        })
        .min_by_key(|b| b.width * b.height)
        .copied()
}

/// The rectangles to fill for this frame's outlines, plus a solid one for
/// whatever is under the mouse.
///
/// Returned rather than drawn here so the caller owns the device: this module
/// knows about layout, not about Direct3D.
pub fn outlines(highlight: Option<usize>) -> (Vec<(i32, i32, i32, i32)>, Vec<(i32, i32, i32, i32)>) {
    let frame = FRAME.lock().unwrap();
    let mut strokes = Vec::with_capacity(frame.len() * 4);
    let mut solid = Vec::new();

    for b in frame.iter() {
        let (x, y, w, h) = (b.x, b.y, b.width, b.height);
        if Some(b.this) == highlight {
            solid.push((x, y, w, h));
            continue;
        }
        strokes.push((x, y, w, STROKE));
        strokes.push((x, y + h - STROKE, w, STROKE));
        strokes.push((x, y, STROKE, h));
        strokes.push((x + w - STROKE, y, STROKE, h));
    }

    (strokes, solid)
}

