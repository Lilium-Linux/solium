//! Frames: each message on the socket, prefixed with its length.
//!
//! A frame is a `u32` byte count, little-endian, then that many bytes: one
//! [`crate::Message`] in the protocol's encoding. A peer reads
//! whole frames out of whatever the socket hands it, which on a
//! non-blocking socket can be half a frame or three and a half.

use std::io::{self, Read, Write};

use crate::Message;
use crate::wire::{DecodeError, Input, Wire};

/// A frame larger than this, in bytes, closes the connection: no message of
/// the protocol is near it (a batch of every window of a full desktop is
/// tens of kilobytes), and a length that large is a broken or hostile peer.
pub const DEFAULT_MAX: usize = 16 * 1024 * 1024;

/// Why the connection cannot go on.
#[derive(Debug)]
pub enum FrameError {
    /// The socket failed.
    Io(io::Error),
    /// The peer closed the connection.
    Closed,
    /// A frame said it was longer than the limit.
    TooLarge(usize),
    /// A frame held something that is not a message.
    Decode(DecodeError),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "the socket failed: {err}"),
            Self::Closed => f.write_str("the other side closed the connection"),
            Self::TooLarge(size) => write!(f, "a frame of {size} bytes is past the limit"),
            Self::Decode(err) => write!(f, "a frame that is not a message: {err}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// A message as one frame, ready to write.
pub fn encode(message: &Message) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    message.encode(&mut out);
    let length = u32::try_from(out.len() - 4).unwrap_or(u32::MAX);
    out[..4].copy_from_slice(&length.to_le_bytes());
    out
}

/// Write one message, whole, to a blocking writer.
pub fn write(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    writer.write_all(&encode(message))
}

/// Frames read so far, and the part of one still arriving.
#[derive(Debug)]
pub struct Reader {
    buffer: Vec<u8>,
    max: usize,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new(DEFAULT_MAX)
    }
}

impl Reader {
    pub fn new(max: usize) -> Self {
        Self {
            buffer: Vec::new(),
            max,
        }
    }

    /// Bytes as they came off the socket.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole message, if one has arrived.
    pub fn next_message(&mut self) -> Result<Option<Message>, FrameError> {
        let Some(header) = self.buffer.get(..4) else {
            return Ok(None);
        };
        let length = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        if length > self.max {
            return Err(FrameError::TooLarge(length));
        }
        let Some(body) = self.buffer.get(4..4 + length) else {
            return Ok(None);
        };
        let message = Input::read_whole::<Message>(body).map_err(FrameError::Decode)?;
        self.buffer.drain(..4 + length);
        Ok(Some(message))
    }

    /// Read what a non-blocking socket has, without waiting, and return the
    /// messages it completed. A closed socket is [`FrameError::Closed`].
    pub fn read_ready(&mut self, socket: &mut impl Read) -> Result<Vec<Message>, FrameError> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match socket.read(&mut chunk) {
                Ok(0) => return Err(FrameError::Closed),
                Ok(count) => self.push(&chunk[..count]),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(FrameError::Io(err)),
            }
        }
        let mut messages = Vec::new();
        while let Some(message) = self.next_message()? {
            messages.push(message);
        }
        Ok(messages)
    }

    /// Block until one whole message has come.
    pub fn read_blocking(&mut self, socket: &mut impl Read) -> Result<Message, FrameError> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            if let Some(message) = self.next_message()? {
                return Ok(message);
            }
            match socket.read(&mut chunk) {
                Ok(0) => return Err(FrameError::Closed),
                Ok(count) => self.push(&chunk[..count]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(FrameError::Io(err)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use super::{FrameError, Reader, encode, write};
    use crate::{Answer, Batch, Change, Command, Event, Message, Rect};

    fn batch() -> Message {
        Message::Batch(Batch {
            serial: 7,
            at: 0.0,
            changes: vec![Change::WorkArea(Rect {
                x: 0.0,
                y: 32.0,
                w: 1920.0,
                h: 1048.0,
            })],
            events: vec![Event::Key("super+t".to_owned()), Event::Relayout],
        })
    }

    #[test]
    fn a_message_crosses_a_socket_whole() {
        let (mut ours, mut theirs) = UnixStream::pair().expect("a socket pair");
        write(&mut ours, &batch()).expect("written");
        let answer = Message::Answer(Answer {
            serial: 7,
            handled: true,
            commands: vec![Command::Unplace(3), Command::Quit],
            motion: None,
            decision: None,
        });
        write(&mut ours, &answer).expect("written");
        let mut reader = Reader::default();
        assert_eq!(
            reader.read_blocking(&mut theirs).expect("a message"),
            batch()
        );
        assert_eq!(
            reader.read_blocking(&mut theirs).expect("a message"),
            answer
        );
    }

    #[test]
    fn a_frame_that_arrives_in_pieces_is_read_once_whole() {
        let bytes = encode(&batch());
        let mut reader = Reader::default();
        for piece in bytes.chunks(3) {
            assert!(
                matches!(reader.next_message(), Ok(None)),
                "nothing until it is whole"
            );
            reader.push(piece);
        }
        assert_eq!(reader.next_message().expect("decodes"), Some(batch()));
        assert!(matches!(reader.next_message(), Ok(None)));
    }

    #[test]
    fn a_frame_past_the_limit_is_refused_before_it_is_read() {
        let mut reader = Reader::new(16);
        reader.push(&1000u32.to_le_bytes());
        assert!(matches!(
            reader.next_message(),
            Err(FrameError::TooLarge(1000))
        ));
    }

    #[test]
    fn a_frame_that_is_not_a_message_is_an_error() {
        let mut reader = Reader::default();
        reader.push(&2u32.to_le_bytes());
        reader.push(&[0x7f, 0x00]);
        assert!(matches!(reader.next_message(), Err(FrameError::Decode(_))));
    }

    #[test]
    fn a_closed_socket_says_so() {
        let (ours, mut theirs) = UnixStream::pair().expect("a socket pair");
        drop(ours);
        theirs.set_nonblocking(true).expect("non-blocking");
        assert!(matches!(
            Reader::default().read_ready(&mut theirs),
            Err(FrameError::Closed)
        ));
    }
}
