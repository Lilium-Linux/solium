//! Protobuf's wire format, for the generated types to be written in and
//! read from: tags, varints, fixed numbers and length-delimited fields.
//!
//! A field is a tag (its number and one of four kinds) and then its value.
//! What a reader does not know it skips by the kind, which is what lets
//! either side be newer than the other.

use std::fmt;

/// Why a value could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended inside a value, or a length ran past its message.
    Truncated,
    /// A number wider than its type.
    Overflow,
    /// A tag whose kind is not one protobuf has now: a group, or nothing.
    BadKind(u32),
    /// A `string` that is not UTF-8, or a `char` that is not a scalar value.
    BadText,
    /// A variant or enum of this name in a case this version does not have:
    /// the peer is newer. A list drops such an item; anywhere else the
    /// value that holds it cannot be read.
    UnknownCase(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the message ended inside a value"),
            Self::Overflow => f.write_str("a number is wider than its type"),
            Self::BadKind(kind) => write!(f, "{kind} is not a kind of field"),
            Self::BadText => f.write_str("text that is not UTF-8"),
            Self::UnknownCase(name) => {
                write!(f, "a `{name}` this version of the protocol does not have")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// How a field's value is laid out, which its tag says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A varint: whole numbers, booleans, enums.
    Varint,
    /// Eight bytes, little-endian: `double`.
    I64,
    /// A length, then that many bytes: text, bytes, a message, a packed list.
    Len,
    /// Four bytes, little-endian: `float`.
    I32,
}

/// Bytes being read, and how far: the message being read ends at `end`.
#[derive(Debug)]
pub struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
    unknown: u32,
}

impl<'a> Input<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            end: bytes.len(),
            unknown: 0,
        }
    }

    /// How many list items were dropped for being in a case this version
    /// does not have.
    pub fn unknown(&self) -> u32 {
        self.unknown
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        if self.at >= self.end {
            return Err(DecodeError::Truncated);
        }
        let byte = *self.bytes.get(self.at).ok_or(DecodeError::Truncated)?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(count).ok_or(DecodeError::Truncated)?;
        if end > self.end {
            return Err(DecodeError::Truncated);
        }
        let taken = self.bytes.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(taken)
    }

    /// A varint: seven bits a byte, low bits first.
    pub fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = self.byte()?;
            let part = u64::from(byte & 0x7f);
            if shift >= 64 || (shift > 0 && part >> (64 - shift) != 0) {
                return Err(DecodeError::Overflow);
            }
            value |= part << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    /// The next field's number and kind, or none at the end of the message.
    pub fn tag(&mut self) -> Result<Option<(u32, Kind)>, DecodeError> {
        if self.at >= self.end {
            return Ok(None);
        }
        let tag = self.varint()?;
        let number = u32::try_from(tag >> 3).map_err(|_| DecodeError::Overflow)?;
        let kind = match tag & 7 {
            0 => Kind::Varint,
            1 => Kind::I64,
            2 => Kind::Len,
            5 => Kind::I32,
            other => return Err(DecodeError::BadKind(other as u32)),
        };
        Ok(Some((number, kind)))
    }

    /// Pass over a field of this kind.
    pub fn skip(&mut self, kind: Kind) -> Result<(), DecodeError> {
        match kind {
            Kind::Varint => {
                self.varint()?;
            }
            Kind::I64 => {
                self.take(8)?;
            }
            Kind::I32 => {
                self.take(4)?;
            }
            Kind::Len => {
                let length = self.length()?;
                self.take(length)?;
            }
        }
        Ok(())
    }

    /// A length, refused when it runs past the message it is in: a length
    /// is never an allocation the input cannot back.
    fn length(&mut self) -> Result<usize, DecodeError> {
        let length = usize::try_from(self.varint()?).map_err(|_| DecodeError::Overflow)?;
        if length > self.end - self.at {
            return Err(DecodeError::Truncated);
        }
        Ok(length)
    }

    /// An unsigned number of at most `bits` bits.
    pub fn unsigned(&mut self, bits: u32) -> Result<u64, DecodeError> {
        let value = self.varint()?;
        if bits < 64 && value >> bits != 0 {
            return Err(DecodeError::Overflow);
        }
        Ok(value)
    }

    /// A signed number of at most `bits` bits, zigzag as `sint32` and
    /// `sint64` are.
    pub fn signed(&mut self, bits: u32) -> Result<i64, DecodeError> {
        let value = unzigzag(self.varint()?);
        if bits < 64 {
            let limit = 1i64 << (bits - 1);
            if value < -limit || value >= limit {
                return Err(DecodeError::Overflow);
            }
        }
        Ok(value)
    }

    pub fn char(&mut self) -> Result<char, DecodeError> {
        let scalar = u32::try_from(self.varint()?).map_err(|_| DecodeError::BadText)?;
        char::from_u32(scalar).ok_or(DecodeError::BadText)
    }

    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        let bytes = self.take(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        let bytes = self.take(8)?;
        Ok(f64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    pub fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        let length = self.length()?;
        Ok(self.take(length)?.to_vec())
    }

    pub fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.length()?;
        std::str::from_utf8(self.take(length)?)
            .map(str::to_owned)
            .map_err(|_| DecodeError::BadText)
    }

    /// A length-delimited field read as a message of its own: `read` sees
    /// its fields and no further, and whatever it leaves is passed over.
    pub fn nested<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<T, DecodeError> {
        let length = self.length()?;
        let outer = self.end;
        let inner = self.at + length;
        self.end = inner;
        let value = read(self);
        self.at = inner;
        self.end = outer;
        value
    }

    /// A field that is a message.
    pub fn message<T: Wire>(&mut self) -> Result<T, DecodeError> {
        self.nested(T::decode)
    }

    /// An item of a list of messages: none, and counted, when it is in a
    /// case this version does not have.
    pub fn item<T: Wire>(&mut self) -> Result<Option<T>, DecodeError> {
        match self.message() {
            Ok(item) => Ok(Some(item)),
            Err(DecodeError::UnknownCase(_)) => {
                self.unknown += 1;
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// A message with no fields this version knows: a variant's case that
    /// carries nothing.
    pub fn unit(&mut self) -> Result<(), DecodeError> {
        self.nested(|input| {
            while let Some((_, kind)) = input.tag()? {
                input.skip(kind)?;
            }
            Ok(())
        })
    }

    /// A packed list: one length, then the items back to back, each read
    /// by `one`.
    pub fn packed(
        &mut self,
        mut one: impl FnMut(&mut Self) -> Result<(), DecodeError>,
    ) -> Result<(), DecodeError> {
        self.nested(|input| {
            while input.at < input.end {
                one(input)?;
            }
            Ok(())
        })
    }

    /// A whole message, and how many list items of it were dropped as
    /// unknown.
    pub fn read_whole<T: Wire>(bytes: &'a [u8]) -> Result<(T, u32), DecodeError> {
        let mut input = Self::new(bytes);
        let value = T::decode(&mut input)?;
        Ok((value, input.unknown))
    }
}

/// A message: a record or a variant of the protocol, written as its fields
/// and read back from them.
pub trait Wire: Sized {
    /// Write this value's fields.
    fn encode(&self, out: &mut Vec<u8>);
    /// Read a value from the fields of the message `input` is in.
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError>;

    /// This value, encoded alone.
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// Write a varint.
pub(crate) fn varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn tag(out: &mut Vec<u8>, number: u32, kind: u64) {
    varint(out, (u64::from(number) << 3) | kind);
}

/// A signed number as `sint32` and `sint64` carry it.
pub(crate) fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// A varint field: a whole number, a boolean or an enum.
pub(crate) fn varint_field(out: &mut Vec<u8>, number: u32, value: u64) {
    tag(out, number, 0);
    varint(out, value);
}

/// A `float` field.
pub(crate) fn f32_field(out: &mut Vec<u8>, number: u32, value: f32) {
    tag(out, number, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

/// A `double` field.
pub(crate) fn f64_field(out: &mut Vec<u8>, number: u32, value: f64) {
    tag(out, number, 1);
    out.extend_from_slice(&value.to_le_bytes());
}

/// A `string` or `bytes` field.
pub(crate) fn bytes_field(out: &mut Vec<u8>, number: u32, value: &[u8]) {
    tag(out, number, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// A length-delimited field whose content `write` writes: a message, or a
/// packed list. The length goes before the content and is known only after
/// it, so one byte is kept for it, and the content moved up when it needs
/// more (128 bytes and over).
pub(crate) fn nested(out: &mut Vec<u8>, number: u32, write: impl FnOnce(&mut Vec<u8>)) {
    tag(out, number, 2);
    delimited(out, write);
}

/// What `write` writes, with its length before it as a varint: a
/// length-delimited field's value, and a frame.
pub(crate) fn delimited(out: &mut Vec<u8>, write: impl FnOnce(&mut Vec<u8>)) {
    let at = out.len();
    out.push(0);
    write(out);
    let length = out.len() - at - 1;
    if length < 0x80 {
        out[at] = length as u8;
    } else {
        let mut prefix = Vec::with_capacity(5);
        varint(&mut prefix, length as u64);
        out.splice(at..=at, prefix);
    }
}

/// A field that is a message.
pub(crate) fn message_field(out: &mut Vec<u8>, number: u32, value: &impl Wire) {
    nested(out, number, |out| value.encode(out));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_written_as_protobuf_writes_them() {
        let mut out = Vec::new();
        // Field 1, a varint of 150: the example protobuf's own guide gives.
        varint_field(&mut out, 1, 150);
        assert_eq!(out, [0x08, 0x96, 0x01]);
        // `sint32` -1 is 1, -2 is 3, 2147483647 is 4294967294.
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(-2), 3);
        assert_eq!(zigzag(2_147_483_647), 4_294_967_294);
        for value in [0, 1, -1, 63, -64, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(value)), value);
        }
        let mut out = Vec::new();
        bytes_field(&mut out, 2, b"testing");
        assert_eq!(out, [0x12, 0x07, b't', b'e', b's', b't', b'i', b'n', b'g']);
    }

    #[test]
    fn a_long_message_gets_a_longer_length_and_reads_back() {
        for length in [0usize, 1, 127, 128, 300, 20_000] {
            let mut out = Vec::new();
            nested(&mut out, 3, |out| {
                out.extend(std::iter::repeat_n(7u8, length))
            });
            let mut input = Input::new(&out);
            assert_eq!(input.tag(), Ok(Some((3, Kind::Len))));
            let read = input.nested(|input| Ok(input.take(length)?.to_vec()));
            assert_eq!(read.map(|bytes| bytes.len()), Ok(length));
            assert_eq!(input.tag(), Ok(None));
        }
    }

    #[test]
    fn a_field_nobody_knows_is_passed_over() {
        let mut out = Vec::new();
        varint_field(&mut out, 9, 5);
        f64_field(&mut out, 10, 1.5);
        f32_field(&mut out, 11, 2.5);
        bytes_field(&mut out, 12, b"later");
        varint_field(&mut out, 1, 42);
        let mut input = Input::new(&out);
        let mut found = None;
        while let Some((number, kind)) = input.tag().expect("a tag") {
            match (number, kind) {
                (1, Kind::Varint) => found = Some(input.varint().expect("a number")),
                (_, kind) => input.skip(kind).expect("skipped"),
            }
        }
        assert_eq!(found, Some(42));
    }

    #[test]
    fn a_bad_message_is_an_error_never_a_panic_or_a_huge_allocation() {
        // A length that the input cannot back.
        let mut input = Input::new(&[0x0a, 0xff, 0xff, 0xff, 0xff, 0x0f]);
        assert_eq!(input.tag(), Ok(Some((1, Kind::Len))));
        assert_eq!(input.bytes(), Err(DecodeError::Truncated));
        // A varint that never ends.
        let mut input = Input::new(&[0xff; 11]);
        assert_eq!(input.varint(), Err(DecodeError::Overflow));
        // A number wider than its type.
        let mut out = Vec::new();
        varint(&mut out, 300);
        assert_eq!(Input::new(&out).unsigned(8), Err(DecodeError::Overflow));
        let mut out = Vec::new();
        varint(&mut out, zigzag(-129));
        assert_eq!(Input::new(&out).signed(8), Err(DecodeError::Overflow));
        // A kind protobuf no longer has.
        assert_eq!(Input::new(&[0x0b]).tag(), Err(DecodeError::BadKind(3)));
        // Text that is not text.
        let mut out = Vec::new();
        bytes_field(&mut out, 1, &[0xff, 0xfe]);
        let mut input = Input::new(&out);
        assert_eq!(input.tag(), Ok(Some((1, Kind::Len))));
        assert_eq!(input.string(), Err(DecodeError::BadText));
        // A message that ends inside a field.
        let mut input = Input::new(&[0x09, 0x00]);
        assert_eq!(input.tag(), Ok(Some((1, Kind::I64))));
        assert_eq!(input.f64(), Err(DecodeError::Truncated));
    }
}
