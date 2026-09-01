//! What the last few packets on a connection were, for when the client stops.
//!
//! A client that freezes sends nothing, and a server with nothing coming in
//! has nothing to log. Every diagnosis so far has come from noticing that a
//! packet was *missing*, which is only possible if the ones around it were
//! written down. This writes them down.
//!
//! # What it catches
//!
//! Both directions, on every connection, in a ring of the last few dozen. The
//! useful part is not any one line but the shape at the end: a client waiting
//! for an answer looks like an inbound packet nothing went out for, followed
//! by silence.
//!
//! That shape is what arms it. The client heartbeats on its own twice a
//! second, so a connection that has been in the world and says nothing for
//! [`QUIET`] has stopped, and the ring is dumped at `WARN` with the whole
//! exchange leading up to it. It dumps once per silence, not once per second.
//!
//! # Watching it live
//!
//! Set `AIKA_TRACE=1` and every packet is logged as it happens, in and out.
//! That is for a reproduction being watched; the ring is for a freeze nobody
//! was watching.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many exchanges are kept. Enough to cover a click and everything it set
/// off, without a connection carrying a log of its whole session.
pub const KEPT: usize = 48;

/// How long a connection that has entered the world may say nothing before it
/// is treated as stopped.
///
/// The client heartbeats twice a second by itself, so this is fifteen missed
/// beats rather than a guess at how patient a player is.
pub const QUIET: Duration = Duration::from_secs(8);

/// Where the header keeps the opcode, which is all that is read back out of
/// an already-encoded frame.
const OPCODE_AT: usize = 6;
/// A frame shorter than the header is not one.
const HEADER: usize = 12;

/// How many bytes of a body are kept beside the opcode.
const HEAD_BYTES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// From the client.
    In,
    /// To the client.
    Out,
}

impl Way {
    fn arrow(self) -> &'static str {
        match self {
            Way::In => "-->",
            Way::Out => "<--",
        }
    }
}

/// One packet, as small as it can be and still be worth reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub at: Duration,
    pub way: Way,
    pub opcode: u16,
    pub size: usize,
    head: [u8; HEAD_BYTES],
    head_len: usize,
    /// Inbound only: whether anything went back out for it. An inbound packet
    /// that is answered by nothing is where a frozen client is usually left
    /// waiting.
    pub answered: bool,
    /// How many of the same thing in a row this stands for.
    ///
    /// A run is folded into one line rather than filling the ring. The first
    /// time this was used the ring held nothing but forty-eight monster
    /// movements broadcast in the same millisecond, and the cast that caused
    /// the freeze had already fallen off the front.
    pub repeats: u32,
}

impl Entry {
    fn line(&self) -> String {
        let bytes: Vec<String> =
            self.head[..self.head_len].iter().map(|b| format!("{b:02X}")).collect();
        let tail = match (self.way, self.answered) {
            (Way::In, false) => "  <- nothing went back",
            _ => "",
        };
        let run = if self.repeats > 1 { format!(" x{}", self.repeats) } else { String::new() };
        format!(
            "  +{:>7.3}s {} 0x{:03x} {:>4}b{}  {}{}",
            self.at.as_secs_f32(),
            self.way.arrow(),
            self.opcode,
            self.size,
            run,
            bytes.join(" "),
            tail
        )
    }
}

/// The last few packets of one connection.
pub struct Trace {
    opened: Instant,
    entries: VecDeque<Entry>,
    /// When the client last said anything.
    heard: Instant,
    /// Whether the current silence has already been reported, so a client that
    /// stopped an hour ago is not announced every eight seconds.
    reported: bool,
    /// Set once the connection is playing. Before that, silence is ordinary:
    /// the client sits on the character screen without a word.
    in_world: bool,
    /// Whether every packet is also logged as it happens.
    live: bool,
}

impl Trace {
    pub fn new(now: Instant) -> Self {
        Self {
            opened: now,
            entries: VecDeque::with_capacity(KEPT),
            heard: now,
            reported: false,
            in_world: false,
            live: std::env::var("AIKA_TRACE").is_ok_and(|v| v != "0"),
        }
    }

    /// Whether every packet is being logged as it happens.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The connection is playing, so silence means something from here on.
    pub fn entered_the_world(&mut self) {
        self.in_world = true;
    }

    fn push(&mut self, entry: Entry) {
        // A run of the same packet is one line with a count. Without this the
        // ring is whatever the noisiest thing on the connection is, and the
        // one packet worth seeing has already fallen off the front.
        if let Some(last) = self.entries.back_mut() {
            if last.way == entry.way
                && last.opcode == entry.opcode
                && last.answered == entry.answered
            {
                last.repeats += 1;
                return;
            }
        }
        if self.entries.len() == KEPT {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    fn entry(&self, way: Way, opcode: u16, body: &[u8], size: usize, now: Instant) -> Entry {
        let head_len = body.len().min(HEAD_BYTES);
        let mut head = [0u8; HEAD_BYTES];
        head[..head_len].copy_from_slice(&body[..head_len]);
        Entry {
            at: now.saturating_duration_since(self.opened),
            way,
            opcode,
            size,
            head,
            head_len,
            answered: true,
            repeats: 1,
        }
    }

    /// A packet from the client. `answered` says whether anything went back.
    pub fn heard_from_client(
        &mut self,
        opcode: u16,
        body: &[u8],
        answered: bool,
        now: Instant,
    ) {
        self.heard = now;
        self.reported = false;
        let mut entry = self.entry(Way::In, opcode, body, body.len(), now);
        entry.answered = answered;
        self.push(entry);
    }

    /// A frame on its way out, still encoded. The opcode is read back out of
    /// the header rather than passed in, so nothing has to remember to.
    pub fn sent_to_client(&mut self, frame: &[u8], now: Instant) {
        let Some(opcode) = opcode_of(frame) else {
            return;
        };
        self.push(self.entry(Way::Out, opcode, &frame[HEADER..], frame.len(), now));
    }

    /// How long the client has been quiet.
    pub fn silence(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.heard)
    }

    /// Whether the client has stopped and this silence has not been reported.
    ///
    /// Only once it is playing: before that a client sitting on the character
    /// screen says nothing for as long as the player takes to choose.
    pub fn has_stopped(&self, now: Instant) -> bool {
        self.in_world && !self.reported && self.silence(now) >= QUIET
    }

    /// Marks the current silence as reported and returns the exchange leading
    /// up to it.
    pub fn report(&mut self, now: Instant) -> String {
        self.reported = true;
        let mut out = format!(
            "the client has said nothing for {:.1}s; the last {} packets were:\n",
            self.silence(now).as_secs_f32(),
            self.entries.len()
        );
        for entry in &self.entries {
            out.push_str(&entry.line());
            out.push('\n');
        }
        if let Some(last) = self.entries.iter().rev().find(|e| e.way == Way::In) {
            if !last.answered {
                out.push_str(&format!(
                    "  the last thing it said was 0x{:03x} and nothing went back for it\n",
                    last.opcode
                ));
            }
        }
        out
    }

    /// One line for a packet, for the live log.
    pub fn last_line(&self) -> String {
        self.entries.back().map(Entry::line).unwrap_or_default()
    }
}

/// The opcode of an encoded frame, or `None` if it is too short to have one.
pub fn opcode_of(frame: &[u8]) -> Option<u16> {
    if frame.len() < HEADER {
        return None;
    }
    Some(u16::from_le_bytes([frame[OPCODE_AT], frame[OPCODE_AT + 1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(opcode: u16, body: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; HEADER];
        out[OPCODE_AT..OPCODE_AT + 2].copy_from_slice(&opcode.to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn trace(now: Instant) -> Trace {
        let mut trace = Trace::new(now);
        trace.live = false;
        trace
    }

    #[test]
    fn the_opcode_comes_out_of_the_header() {
        assert_eq!(opcode_of(&frame(0x320, &[1, 2, 3])), Some(0x320));
        assert_eq!(opcode_of(&[0u8; 11]), None, "too short to hold a header");
    }

    #[test]
    fn it_keeps_the_last_few_and_forgets_the_rest() {
        let start = Instant::now();
        let mut trace = trace(start);
        for i in 0..(KEPT as u16 + 10) {
            trace.heard_from_client(i, &[], true, start);
        }
        assert_eq!(trace.entries.len(), KEPT);
        assert_eq!(trace.entries.front().unwrap().opcode, 10, "it kept the oldest");
        assert_eq!(trace.entries.back().unwrap().opcode, KEPT as u16 + 9);
    }

    /// A run of the same packet is one line with a count, or the ring holds
    /// whatever the noisiest thing on the connection is and nothing else. The
    /// first report this tool ever produced was forty-eight monster movements
    /// broadcast in the same millisecond; the cast that caused the freeze had
    /// already fallen off the front.
    #[test]
    fn a_run_of_the_same_packet_does_not_fill_the_ring() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.entered_the_world();

        trace.heard_from_client(0x320, &[], true, start);
        for _ in 0..200 {
            trace.sent_to_client(&frame(0x301, &[1, 2]), start);
        }
        trace.heard_from_client(0x305, &[], false, start);

        assert_eq!(trace.entries.len(), 3, "the run was not folded into one");
        assert_eq!(trace.entries[1].repeats, 200);

        let report = trace.report(start + QUIET);
        assert!(report.contains("0x320"), "what mattered fell off the front:
{report}");
        assert!(report.contains("x200"), "the run is not counted:
{report}");
    }

    /// Silence before the character is in the world is a player choosing one,
    /// not a client that has stopped.
    #[test]
    fn silence_on_the_character_screen_is_not_a_freeze() {
        let start = Instant::now();
        let mut trace = trace(start);
        let later = start + QUIET * 2;

        assert!(!trace.has_stopped(later), "it called the selection screen a freeze");
        trace.entered_the_world();
        assert!(trace.has_stopped(later));
    }

    #[test]
    fn a_client_that_speaks_again_is_not_stopped() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.entered_the_world();
        let later = start + QUIET * 2;
        trace.heard_from_client(0x301, &[], true, later);

        assert!(!trace.has_stopped(later), "it had just spoken");
        assert!(trace.has_stopped(later + QUIET));
    }

    /// Reported once, not once per check, or a client that stopped an hour ago
    /// fills the log with the same forty-eight lines.
    #[test]
    fn one_silence_is_reported_once() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.entered_the_world();
        let later = start + QUIET;

        assert!(trace.has_stopped(later));
        let _ = trace.report(later);
        assert!(!trace.has_stopped(later + QUIET), "it reported the same silence twice");

        // and a client that comes back and stops again is reported again
        trace.heard_from_client(0x301, &[], true, later + QUIET);
        assert!(trace.has_stopped(later + QUIET * 3));
    }

    /// The whole point: the report says which packet was left hanging.
    #[test]
    fn the_report_names_the_packet_nothing_answered() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.entered_the_world();
        trace.heard_from_client(0x30f, &[1, 2], true, start);
        trace.sent_to_client(&frame(0x110, &[]), start);
        trace.heard_from_client(0x31d, &[7, 0], false, start);

        let report = trace.report(start + QUIET);
        assert!(report.contains("0x30f"), "the exchange before it is missing");
        assert!(
            report.contains("the last thing it said was 0x31d and nothing went back for it"),
            "it did not name the unanswered packet:\n{report}"
        );
    }

    /// And it stays quiet about a client whose last word was answered, so the
    /// line means something when it does appear.
    #[test]
    fn a_last_packet_that_was_answered_is_not_blamed() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.entered_the_world();
        trace.heard_from_client(0x31d, &[], true, start);

        let report = trace.report(start + QUIET);
        assert!(!report.contains("nothing went back for it"), "it blamed an answered packet");
    }

    #[test]
    fn a_frame_too_short_to_have_an_opcode_is_not_recorded() {
        let start = Instant::now();
        let mut trace = trace(start);
        trace.sent_to_client(&[0u8; 4], start);
        assert!(trace.entries.is_empty());
    }
}
