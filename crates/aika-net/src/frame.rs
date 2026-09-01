//! Protocol framing: cutting frames out of the TCP stream and encoding or
//! decoding the message header.
//!
//! Layout of a deciphered message:
//!
//! ```text
//! offset 0..4    frame header (size/checksum/seed) - see crypto.rs
//! offset 4..6    u16  sender id (the connection or unit the message is from)
//! offset 6..8    u16  opcode
//! offset 8..12   u32  sender timestamp
//! offset 12..    body, layout depends on the opcode
//! ```
//!
//! Unlike the original, which reads one frame per socket read, this reader is
//! sans-io: feed it bytes in whatever order they arrive and it hands back
//! complete messages, coping with split frames and with several frames in one
//! read. TCP gives no promise that a read is a frame, and the original gets
//! away with it only because the packets are small.

use crate::crypto;

/// Smallest valid frame: 4 header bytes plus sender, opcode and timestamp.
pub const MIN_FRAME: usize = 12;

/// Prefix the client sends at the start of a connection, which the server
/// discards. The original cuts four bytes blindly; we check all four and
/// decide from the content, so a client that sends no prefix still works.
const CLIENT_HELLO: [u8; 4] = [0x11, 0xF3, 0x11, 0x1F];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub sender: u16,
    pub opcode: u16,
    pub time: u32,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The header checksum disagrees with the deciphered content.
    BadChecksum,
    /// Size field below the minimum: the stream is corrupt and the connection
    /// should be dropped.
    BadLength(u16),
}

/// Accumulates bytes read from the socket and yields complete messages.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Next complete message, if enough bytes have arrived. Call in a loop
    /// until it returns `None` after every `push`.
    pub fn next_message(&mut self) -> Option<Result<Message, FrameError>> {
        if self.buf.starts_with(&CLIENT_HELLO) {
            self.buf.drain(..CLIENT_HELLO.len());
        }
        if self.buf.len() < MIN_FRAME {
            return None;
        }

        let size = u16::from_le_bytes([self.buf[0], self.buf[1]]) as usize;
        if size < MIN_FRAME {
            return Some(Err(FrameError::BadLength(size as u16)));
        }
        if self.buf.len() < size {
            return None;
        }

        let mut frame: Vec<u8> = self.buf.drain(..size).collect();
        if !crypto::decrypt(&mut frame) {
            return Some(Err(FrameError::BadChecksum));
        }

        Some(Ok(Message {
            sender: u16::from_le_bytes([frame[4], frame[5]]),
            opcode: u16::from_le_bytes([frame[6], frame[7]]),
            time: u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]),
            body: frame[12..].to_vec(),
        }))
    }
}

/// Builds and enciphers a frame ready to be written to the socket.
pub fn encode(msg: &Message, seed: u8) -> Vec<u8> {
    let mut frame = Vec::with_capacity(MIN_FRAME + msg.body.len());
    frame.extend_from_slice(&[0u8; 4]); // header, filled in by encrypt
    frame.extend_from_slice(&msg.sender.to_le_bytes());
    frame.extend_from_slice(&msg.opcode.to_le_bytes());
    frame.extend_from_slice(&msg.time.to_le_bytes());
    frame.extend_from_slice(&msg.body);
    crypto::encrypt(&mut frame, seed);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(opcode: u16, body: &[u8]) -> Message {
        Message { sender: 0x7530, opcode, time: 0xDEAD_BEEF, body: body.to_vec() }
    }

    #[test]
    fn roundtrip_single_frame() {
        let original = msg(0x3001, b"aika-rs!");
        let wire = encode(&original, 0x5A);

        let mut reader = FrameReader::new();
        reader.push(&wire);
        let got = reader.next_message().unwrap().unwrap();
        assert_eq!(got, original);
        assert!(reader.next_message().is_none());
    }

    #[test]
    fn reassembles_fragmented_frame() {
        let original = msg(0x1002, &[9u8; 40]);
        let wire = encode(&original, 0x01);

        let mut reader = FrameReader::new();
        for chunk in wire.chunks(5) {
            reader.push(chunk);
        }
        assert_eq!(reader.next_message().unwrap().unwrap(), original);
    }

    #[test]
    fn splits_two_frames_in_one_read() {
        let a = msg(0x0001, b"1234");
        let b = msg(0x0002, b"abcdefgh");
        let mut wire = encode(&a, 0x10);
        wire.extend(encode(&b, 0x20));

        let mut reader = FrameReader::new();
        reader.push(&wire);
        assert_eq!(reader.next_message().unwrap().unwrap(), a);
        assert_eq!(reader.next_message().unwrap().unwrap(), b);
        assert!(reader.next_message().is_none());
    }

    #[test]
    fn strips_client_hello_prefix() {
        let original = msg(0x0DA5, b"hello");
        let mut wire = vec![0x11, 0xF3, 0x11, 0x1F];
        wire.extend(encode(&original, 0x77));

        let mut reader = FrameReader::new();
        reader.push(&wire);
        assert_eq!(reader.next_message().unwrap().unwrap(), original);
    }

    #[test]
    fn corrupted_seed_reports_bad_checksum() {
        // The checksum only covers seed and length (see crypto.rs), so we
        // corrupt the seed byte to trigger the failure.
        let mut wire = encode(&msg(0x3001, &[1, 2, 3, 4, 5, 6, 7, 8]), 0x42);
        wire[3] ^= 0xFF;

        let mut reader = FrameReader::new();
        reader.push(&wire);
        assert_eq!(reader.next_message().unwrap().unwrap_err(), FrameError::BadChecksum);
    }
}
