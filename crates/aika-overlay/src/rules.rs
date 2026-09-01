//! Which windows to hide, and where to put the ones that stay.
//!
//! The rules live in `aika-overlay.rules`, beside the library, so changing the
//! interface is editing a text file rather than rebuilding: the loop that
//! matters here is "move it, look at it, move it again", and a compile in the
//! middle of that loop is what stops anyone from doing it twenty times.
//!
//! # Why a widget is named by its size
//!
//! An address cannot name a window: it is different on every run. What holds
//! still is the shape the interface was designed with — the bottom bar is
//! 1023 by 63 in every session, because that is what its author drew. So a
//! rule matches on class and size, both of which come straight out of the
//! object, and both of which a snapshot (`F10`) prints.
//!
//! ```text
//! # class       w     h     x    y    shift
//! hide  SPanel   1023  63    *    *
//! move  SPanel   676   232   302  140   +0  -40
//! ```
//!
//! `x` and `y` are the position the widget already has, and `*` matches any —
//! which is what a rule wants when it means "every button of this shape" and
//! very much not what it wants otherwise.
//!
//! `hide` stops the widget being drawn — the client keeps it, updates it and
//! considers it there, so nothing downstream notices. `move` shifts it by an
//! offset every frame, which is what makes it stick: the client rewrites the
//! position whenever it feels like it, and arguing once would last one frame.

use crate::log;

/// A widget is matched by what it is, how big it is and where it sits, never by
/// where it happens to live in memory this time.
///
/// Size alone was not enough and the action bar is why: all sixty-six of its
/// buttons are 101 by 22, so a rule naming that size named all sixty-six. The
/// position is what separates them, and `None` — written `*` — is what asks for
/// the old behaviour of matching every widget of a shape.
#[derive(Clone, Copy, PartialEq)]
pub struct Rule {
    pub class: &'static str,
    pub width: i16,
    pub height: i16,
    /// Position relative to the parent, as the widget stores it. `None` matches
    /// any.
    pub at: (Option<i16>, Option<i16>),
    pub hide: bool,
    /// How far to shift it, in pixels, relative to where the client put it.
    pub shift: (i16, i16),
}

/// Where the position sits inside a widget, as two `i16`s.
///
/// Found by taking a snapshot of every widget on screen and looking for the
/// field whose values spread across the screen: `+0x3C` held 118 distinct
/// pairs across 178 widgets, x from -20 to 997 and y from -95 to 705 on a
/// 1024 by 768 screen. The negatives are children sitting above their parent's
/// origin, which is also what says the position is relative to the parent.
pub const POSITION: usize = 0x3C;

/// And the size, in the very next word — 1024 by 768 on the root panel, 101 by
/// 22 on all sixty-six buttons of the action bar.
///
/// The same four numbers exist again as `f32` at `+0x5C` through `+0x68`, which
/// is how they were confirmed: one login panel reads `(530, 434)` and
/// `(220, 158)` as integers and `530.0, 434.0, 220.0, 158.0` as floats, in that
/// order. The integers are written to, being the pair the layout is expressed
/// in; whether the client rebuilds the floats from them every frame or the
/// other way round has not mattered yet.
pub const SIZE: usize = 0x40;

/// The widget this one sits inside, or null at the root.
///
/// A position is relative to the parent, so a rectangle on the screen is the
/// sum of the chain. Found in the dump: two panels of the login screen both
/// carry `0x3313E7B8` here, and that is the address of the one `SControl` that
/// every snapshot contains exactly once — the root.
pub const PARENT: usize = 0x04;

/// How far up the chain to walk before deciding a pointer is not a widget.
///
/// The interface is nested perhaps five deep; anything past this is a loop or a
/// field that only looks like a pointer, and following it would be reading
/// memory at an address nobody promised.
const MAX_DEPTH: usize = 16;

/// Rough bounds for a heap pointer in this process. Not a guarantee — nothing
/// cheap is — but it rejects the small integers and the sentinel values that
/// share the field's neighbourhood, which is what a wild read would follow.
fn plausible(pointer: usize) -> bool {
    (0x0001_0000..0x7FFF_0000).contains(&pointer) && pointer % 4 == 0
}

/// Where a widget actually sits on the screen: its own position plus every
/// parent's, and its own size.
pub unsafe fn absolute_rect(this: *const u8) -> Option<(i32, i32, i32, i32)> {
    let size = this.add(SIZE) as *const i16;
    let (width, height) = (*size as i32, *size.add(1) as i32);
    if width <= 0 || height <= 0 {
        return None;
    }

    let (mut x, mut y) = (0i32, 0i32);
    let mut node = this;
    for _ in 0..MAX_DEPTH {
        let position = node.add(POSITION) as *const i16;
        x += *position as i32;
        y += *position.add(1) as i32;

        let parent = *(node.add(PARENT) as *const usize);
        if !plausible(parent) {
            break;
        }
        node = parent as *const u8;
    }

    Some((x, y, width, height))
}

static mut RULES: Vec<Rule> = Vec::new();

/// Reads the rules file. Called at startup and whenever `F11` asks for it.
///
/// A missing file is not an error: it is what an install with nothing to
/// change looks like, and the overlay has to run in that state.
pub fn reload(path: &std::path::Path) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => {
            log(&format!(
                "[rules] no {} yet; the interface is left as the client draws it",
                path.display()
            ));
            unsafe { RULES = Vec::new() };
            return;
        }
    };

    let mut parsed = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        match parse(line) {
            Some(rule) => parsed.push(rule),
            None => log(&format!("[rules] line {}: cannot read {line:?}", number + 1)),
        }
    }

    log(&format!("[rules] {} rule(s) loaded", parsed.len()));
    for rule in &parsed {
        log(&format!(
            "[rules]   {} {}x{} at {},{}{}{}",
            rule.class,
            rule.width,
            rule.height,
            rule.at.0.map_or("*".to_string(), |v| v.to_string()),
            rule.at.1.map_or("*".to_string(), |v| v.to_string()),
            if rule.hide { " hidden" } else { "" },
            if rule.shift != (0, 0) {
                format!(" shifted by {},{}", rule.shift.0, rule.shift.1)
            } else {
                String::new()
            }
        ));
    }
    unsafe { RULES = parsed };
}

/// A leaked `&'static str`, because a rule outlives the text it was read from
/// and the alternative is an owned `String` in a static that is read from the
/// render thread. There are a handful of rules and they are re-read by hand, so
/// what leaks is bounded by how often somebody presses a key.
fn parse(line: &str) -> Option<Rule> {
    fn number(text: &str) -> Option<i16> {
        text.trim_start_matches('+').parse().ok()
    }
    /// `*` means "any", which is how a rule addresses every widget of a shape.
    fn maybe(text: &str) -> Option<Option<i16>> {
        if text == "*" {
            Some(None)
        } else {
            number(text).map(Some)
        }
    }

    let mut parts = line.split_whitespace();
    let action = parts.next()?;
    let class: &'static str = Box::leak(parts.next()?.to_string().into_boxed_str());
    let width = number(parts.next()?)?;
    let height = number(parts.next()?)?;
    let at = (maybe(parts.next()?)?, maybe(parts.next()?)?);

    match action {
        "hide" => Some(Rule { class, width, height, at, hide: true, shift: (0, 0) }),
        "move" => {
            let dx = number(parts.next()?)?;
            let dy = number(parts.next()?)?;
            Some(Rule { class, width, height, at, hide: false, shift: (dx, dy) })
        }
        _ => None,
    }
}

/// The rule for a widget, if any.
///
/// Reading the size out of the object rather than being told it: the caller
/// has the object, and going through the same field the rules match on is what
/// keeps the two from drifting apart.
pub unsafe fn matching(class: &str, this: *const u8) -> Option<Rule> {
    let size = this.add(SIZE) as *const i16;
    let (width, height) = (*size, *size.add(1));

    let position = this.add(POSITION) as *const i16;
    let (x, y) = (*position, *position.add(1));

    #[allow(static_mut_refs)]
    RULES
        .iter()
        .find(|r| {
            r.class == class
                && r.width == width
                && r.height == height
                && r.at.0.is_none_or(|want| want == x)
                && r.at.1.is_none_or(|want| want == y)
        })
        .copied()
}

/// Adds a `move` line to the rules file, so a drag survives the session.
///
/// Appended rather than rewritten: the file is something a person edits by
/// hand, with their own comments and their own order, and a tool that
/// rearranges it to suit itself is a tool people stop using.
pub fn append_move(path: &std::path::Path, rule: &Rule) {
    use std::io::Write;
    let line = format!(
        "move  {}  {}  {}  {}  {}  {}  {}
",
        rule.class,
        rule.width,
        rule.height,
        rule.at.0.map_or("*".to_string(), |v| v.to_string()),
        rule.at.1.map_or("*".to_string(), |v| v.to_string()),
        rule.shift.0,
        rule.shift.1
    );
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    match written {
        Ok(()) => log(&format!("[rules] saved: {}", line.trim_end())),
        Err(e) => log(&format!("[rules] could not save: {e}")),
    }
}

/// Shifts a widget by the rule's offset, and says what was there before.
///
/// The original is handed back so the caller can put it straight after the
/// client has drawn: leaving the moved value in place would have the client
/// read its own position, find it moved, and shift it again on the next frame
/// until the window walked off the screen.
pub unsafe fn shift(this: *mut u8, by: (i16, i16)) -> (i16, i16) {
    let position = this.add(POSITION) as *mut i16;
    let previous = (*position, *position.add(1));
    *position = previous.0.wrapping_add(by.0);
    *position.add(1) = previous.1.wrapping_add(by.1);
    previous
}

/// Puts a position back exactly as it was.
pub unsafe fn restore(this: *mut u8, to: (i16, i16)) {
    let position = this.add(POSITION) as *mut i16;
    *position = to.0;
    *position.add(1) = to.1;
}
