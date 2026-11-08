//! The encoding of WIT values, for the types the generated code builds on.

use std::fmt;

/// Why a value could not be read: the input ended, or held something its
/// type cannot be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended inside a value.
    Truncated,
    /// A LEB128 number ran past its type's width.
    Overflow,
    /// A `bool` or `option` tag that is not 0 or 1.
    BadTag(u8),
    /// A `variant` or `enum` case past the last one, for the named type.
    BadCase(&'static str, u64),
    /// A `string` that is not UTF-8, or a `char` that is not a scalar value.
    BadText,
    /// Bytes left over after a whole message.
    Trailing(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the message ended inside a value"),
            Self::Overflow => f.write_str("a number is wider than its type"),
            Self::BadTag(tag) => write!(f, "{tag} is not a flag (0 or 1)"),
            Self::BadCase(name, case) => write!(f, "{name} has no case {case}"),
            Self::BadText => f.write_str("text that is not UTF-8"),
            Self::Trailing(left) => write!(f, "{left} bytes after the message"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Bytes being read, and how far.
#[derive(Debug)]
pub struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// How many bytes are left.
    pub fn left(&self) -> usize {
        self.bytes.len() - self.at
    }

    pub fn byte(&mut self) -> Result<u8, DecodeError> {
        let byte = *self.bytes.get(self.at).ok_or(DecodeError::Truncated)?;
        self.at += 1;
        Ok(byte)
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(count).ok_or(DecodeError::Truncated)?;
        let taken = self.bytes.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(taken)
    }

    /// An unsigned LEB128 number of at most `bits` bits.
    pub fn leb(&mut self, bits: u32) -> Result<u64, DecodeError> {
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
                break;
            }
            shift += 7;
        }
        if bits < 64 && value >> bits != 0 {
            return Err(DecodeError::Overflow);
        }
        Ok(value)
    }

    /// The length of a list or a string, refused when it cannot fit in
    /// what is left: a length is never an allocation the input cannot back.
    pub fn length(&mut self) -> Result<usize, DecodeError> {
        let length = usize::try_from(self.leb(64)?).map_err(|_| DecodeError::Overflow)?;
        if length > self.left() {
            return Err(DecodeError::Truncated);
        }
        Ok(length)
    }

    /// A whole message: its value, and nothing after it.
    pub fn read_whole<T: Wire>(bytes: &'a [u8]) -> Result<T, DecodeError> {
        let mut input = Self::new(bytes);
        let value = T::decode(&mut input)?;
        match input.left() {
            0 => Ok(value),
            left => Err(DecodeError::Trailing(left)),
        }
    }
}

/// A value that can be written in the protocol's encoding and read back.
pub trait Wire: Sized {
    fn encode(&self, out: &mut Vec<u8>);
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError>;

    /// This value, encoded alone.
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// Write an unsigned LEB128 number.
pub(crate) fn write_leb(out: &mut Vec<u8>, mut value: u64) {
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

fn zigzag(value: i64) -> u64 {
    #[expect(clippy::cast_sign_loss, reason = "zigzag's own bit pattern")]
    let encoded = ((value << 1) ^ (value >> 63)) as u64;
    encoded
}

fn unzigzag(value: u64) -> i64 {
    #[expect(clippy::cast_possible_wrap, reason = "zigzag's own bit pattern")]
    let decoded = ((value >> 1) as i64) ^ -((value & 1) as i64);
    decoded
}

impl Wire for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        match input.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            tag => Err(DecodeError::BadTag(tag)),
        }
    }
}

macro_rules! unsigned {
    ($($ty:ty),*) => {$(
        impl Wire for $ty {
            fn encode(&self, out: &mut Vec<u8>) {
                write_leb(out, u64::from(*self));
            }
            fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
                <$ty>::try_from(input.leb(<$ty>::BITS)?).map_err(|_| DecodeError::Overflow)
            }
        }
    )*};
}
unsigned!(u8, u16, u32, u64);

macro_rules! signed {
    ($($ty:ty),*) => {$(
        impl Wire for $ty {
            fn encode(&self, out: &mut Vec<u8>) {
                write_leb(out, zigzag(i64::from(*self)));
            }
            fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
                <$ty>::try_from(unzigzag(input.leb(64)?)).map_err(|_| DecodeError::Overflow)
            }
        }
    )*};
}
signed!(i8, i16, i32, i64);

impl Wire for f32 {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        let bytes = input.take(4)?;
        Ok(Self::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3],
        ]))
    }
}

impl Wire for f64 {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        let bytes = input.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(Self::from_le_bytes(array))
    }
}

impl Wire for char {
    fn encode(&self, out: &mut Vec<u8>) {
        write_leb(out, u64::from(u32::from(*self)));
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        let scalar = u32::decode(input)?;
        Self::from_u32(scalar).ok_or(DecodeError::BadText)
    }
}

impl Wire for String {
    fn encode(&self, out: &mut Vec<u8>) {
        write_leb(out, self.len() as u64);
        out.extend_from_slice(self.as_bytes());
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        let length = input.length()?;
        let bytes = input.take(length)?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| DecodeError::BadText)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        write_leb(out, self.len() as u64);
        for item in self {
            item.encode(out);
        }
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        // Every item takes at least one byte, so `length` bounds the
        // allocation by the input it came in.
        let length = input.length()?;
        let mut items = Self::with_capacity(length);
        for _ in 0..length {
            items.push(T::decode(input)?);
        }
        Ok(items)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                value.encode(out);
            }
        }
    }
    fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
        match input.byte()? {
            0 => Ok(None),
            1 => T::decode(input).map(Some),
            tag => Err(DecodeError::BadTag(tag)),
        }
    }
}

macro_rules! tuple {
    ($($name:ident),+) => {
        impl<$($name: Wire),+> Wire for ($($name,)+) {
            #[allow(non_snake_case, reason = "the type parameters, as names")]
            fn encode(&self, out: &mut Vec<u8>) {
                let ($($name,)+) = self;
                $($name.encode(out);)+
            }
            fn decode(input: &mut Input<'_>) -> Result<Self, DecodeError> {
                Ok(($($name::decode(input)?,)+))
            }
        }
    };
}
tuple!(A, B);
tuple!(A, B, C);
tuple!(A, B, C, D);

#[cfg(test)]
mod tests {
    use super::{DecodeError, Input, Wire, write_leb};

    fn round_trip<T: Wire + PartialEq + std::fmt::Debug>(value: T) {
        let bytes = value.to_bytes();
        assert_eq!(Input::read_whole::<T>(&bytes), Ok(value), "{bytes:?}");
    }

    #[test]
    fn every_primitive_reads_back_what_was_written() {
        round_trip(true);
        round_trip(0u8);
        round_trip(u64::MAX);
        round_trip(i64::MIN);
        round_trip(-1i32);
        round_trip(1.5f64);
        round_trip(f32::INFINITY);
        round_trip('я');
        round_trip("Firefox — привет".to_owned());
        round_trip(vec![Some(1u32), None, Some(3)]);
        round_trip((7u64, "x".to_owned()));
    }

    #[test]
    fn the_encoding_is_the_documented_one() {
        assert_eq!(300u32.to_bytes(), [0xac, 0x02], "LEB128");
        assert_eq!((-2i32).to_bytes(), [0x03], "zigzag, then LEB128");
        assert_eq!("ab".to_owned().to_bytes(), [2, b'a', b'b']);
        assert_eq!(Some(true).to_bytes(), [1, 1]);
        assert_eq!(1.0f64.to_bytes(), 1.0f64.to_le_bytes());
    }

    #[test]
    fn a_bad_message_is_an_error_never_a_panic_or_a_huge_allocation() {
        assert_eq!(
            Input::read_whole::<u32>(&[0x80]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(
            Input::read_whole::<u8>(&[0x80, 0x02]),
            Err(DecodeError::Overflow)
        );
        assert_eq!(Input::read_whole::<bool>(&[2]), Err(DecodeError::BadTag(2)));
        let mut huge = Vec::new();
        write_leb(&mut huge, u64::MAX >> 1);
        assert_eq!(
            Input::read_whole::<Vec<u8>>(&huge),
            Err(DecodeError::Truncated)
        );
        assert_eq!(
            Input::read_whole::<String>(&[2, 0xff, 0xfe]),
            Err(DecodeError::BadText)
        );
        assert_eq!(
            Input::read_whole::<u8>(&[1, 2]),
            Err(DecodeError::Trailing(1))
        );
        for length in 0..12 {
            let bytes = vec![0xffu8; length];
            let _ = Input::read_whole::<Vec<String>>(&bytes);
            let _ = Input::read_whole::<(u64, Option<f64>, String)>(&bytes);
        }
    }
}
