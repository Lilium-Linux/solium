//! Frames: each message on the socket, prefixed with its length.
//!
//! A frame is a byte count as a varint, then that many bytes: one
//! [`crate::Message`] as protobuf writes it. This is protobuf's own
//! "delimited" framing, which its libraries read and write whole
//! (`ParseDelimitedFrom`, `sizeDelimitedDecodeStream`, `protodelim`). A peer
//! reads whole frames out of whatever the socket hands it, which on a
//! non-blocking socket can be half a frame or three and a half.

use std::io::{self, Read, Write};

use crate::Message;
use crate::wire::{self, DecodeError, Input, Wire};

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
    let mut out = Vec::with_capacity(256);
    wire::delimited(&mut out, |out| message.encode(out));
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
    /// What was left out of the messages read, for being of a kind this
    /// version does not have.
    unknown: u32,
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
            unknown: 0,
        }
    }

    /// Bytes as they came off the socket.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// How much of what has been read since this was last asked was left
    /// out for being of a kind this version does not have: a command, a
    /// change or a whole message a newer peer sent. The rest was read.
    pub fn unknown(&mut self) -> u32 {
        std::mem::take(&mut self.unknown)
    }

    /// The next whole message, if one has arrived. One of a kind this
    /// version does not have is passed over and counted.
    pub fn next_message(&mut self) -> Result<Option<Message>, FrameError> {
        loop {
            // The length: a varint of at most five bytes, since no frame
            // is near four gigabytes.
            let mut length = 0u64;
            let mut header = 0;
            loop {
                let Some(byte) = self.buffer.get(header) else {
                    return Ok(None);
                };
                length |= u64::from(byte & 0x7f) << (7 * header);
                header += 1;
                if byte & 0x80 == 0 {
                    break;
                }
                if header == 5 {
                    return Err(FrameError::TooLarge(usize::MAX));
                }
            }
            let length = usize::try_from(length).unwrap_or(usize::MAX);
            if length > self.max {
                return Err(FrameError::TooLarge(length));
            }
            let Some(body) = self.buffer.get(header..header + length) else {
                return Ok(None);
            };
            let read = Input::read_whole::<Message>(body);
            self.buffer.drain(..header + length);
            match read {
                Ok((message, unknown)) => {
                    self.unknown += unknown;
                    return Ok(Some(message));
                }
                Err(DecodeError::UnknownCase(_)) => self.unknown += 1,
                Err(err) => return Err(FrameError::Decode(err)),
            }
        }
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
        // 1000, as a varint.
        reader.push(&[0xe8, 0x07]);
        assert!(matches!(
            reader.next_message(),
            Err(FrameError::TooLarge(1000))
        ));
    }

    #[test]
    fn a_frame_that_is_not_a_message_is_an_error() {
        let mut reader = Reader::default();
        // Two bytes: a field of a kind protobuf does not have.
        reader.push(&[2, 0x0b, 0x00]);
        assert!(matches!(reader.next_message(), Err(FrameError::Decode(_))));
    }

    /// A newer peer may send a message this version has no case for. It is
    /// not the end of the connection: the frame is passed over, and what
    /// comes after it is read.
    #[test]
    fn a_message_of_a_kind_this_version_does_not_have_is_passed_over() {
        let mut reader = Reader::default();
        // One frame of three bytes: field 99 of `message`, a varint.
        reader.push(&[3, 0x98, 0x06, 0x01]);
        reader.push(&encode(&batch()));
        assert_eq!(reader.next_message().expect("decodes"), Some(batch()));
        assert_eq!(reader.unknown(), 1, "and it is counted, to be said");
        assert_eq!(reader.unknown(), 0, "once");
    }

    #[test]
    fn a_frame_is_its_length_as_a_varint_and_then_the_message() {
        let short = encode(&Message::Goodbye("bye".to_owned()));
        // `goodbye` is `message`'s tenth case: field 10, text.
        assert_eq!(short, [5, 0x52, 3, b'b', b'y', b'e']);
        let long = encode(&Message::Goodbye("x".repeat(300)));
        // 303 bytes of message: 0xaf 0x02.
        assert_eq!(long[..2], [0xaf, 0x02]);
        assert_eq!(long.len(), 2 + 303);
        let mut reader = Reader::default();
        reader.push(&long);
        assert_eq!(
            reader.next_message().expect("decodes"),
            Some(Message::Goodbye("x".repeat(300)))
        );
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
