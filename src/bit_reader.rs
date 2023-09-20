#![forbid(unsafe_code)]

use std::io::{self, BufRead};

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BitSequence {
    bits: u16,
    len: u8,
}

impl BitSequence {
    pub fn new(mut bits: u16, len: u8) -> Self {
        assert!(len <= 16);
        if len < 16 {
            bits &= !(!0u16 << len);
        }
        Self { bits, len }
    }

    pub fn bits(&self) -> u16 {
        self.bits
    }

    pub fn len(&self) -> u8 {
        self.len
    }

    pub fn concat(self, other: Self) -> Self {
        assert!(other.len() + self.len <= 16);
        Self {
            bits: (self.bits << other.len()) | other.bits,
            len: self.len + other.len,
        }
    }

    pub fn consume(&mut self, len: u8) -> Self {
        assert!(self.len >= len);

        let bits = self.bits & !(!0 << len);
        self.len -= len;
        self.bits >>= len;

        BitSequence::new(bits, len)
    }
}

////////////////////////////////////////////////////////////////////////////////

pub struct BitReader<T> {
    stream: T,
    remainder: BitSequence,
}

impl<T: BufRead> BitReader<T> {
    pub fn new(stream: T) -> Self {
        Self {
            stream,
            remainder: BitSequence::new(0, 0),
        }
    }

    pub fn read_bits(&mut self, len: u8) -> io::Result<BitSequence> {
        assert!(len <= 16 && len != 0);
        if self.remainder.len() >= len {
            return Ok(self.remainder.consume(len));
        }

        let to_fill: u8 = len - self.remainder.len();

        let mut byte = vec![0u8; ((to_fill - 1) / 8 + 1).into()];
        self.stream.read_exact(&mut byte)?;
        let mut bits = if byte.len() == 1 {
            BitSequence::new(byte[0].into(), 8)
        } else {
            BitSequence::new(((byte[1] as u16) << 8) + byte[0] as u16, 16)
        };

        let to_read = bits.consume(to_fill).concat(self.remainder);
        self.remainder = bits;

        Ok(to_read)
    }

    /// Discard all the unread bits in the current byte and return a mutable reference
    /// to the underlying reader.
    pub fn borrow_reader_from_boundary(&mut self) -> &mut T {
        assert!(self.remainder.len() <= 8);
        self.remainder.consume(self.remainder.len());
        &mut self.stream
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::ReadBytesExt;

    #[test]
    fn read_bits() -> io::Result<()> {
        let data: &[u8] = &[0b01100011, 0b01011011, 0b10101111];
        let mut reader = BitReader::new(data);
        assert_eq!(reader.read_bits(1)?, BitSequence::new(0b1, 1));
        assert_eq!(reader.read_bits(2)?, BitSequence::new(0b01, 2));
        assert_eq!(reader.read_bits(3)?, BitSequence::new(0b100, 3));
        assert_eq!(reader.read_bits(4)?, BitSequence::new(0b1101, 4));
        assert_eq!(reader.read_bits(5)?, BitSequence::new(0b10110, 5));
        assert_eq!(reader.read_bits(8)?, BitSequence::new(0b01011110, 8));
        assert_eq!(
            reader.read_bits(2).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        Ok(())
    }

    #[test]
    fn borrow_reader_from_boundary() -> io::Result<()> {
        let data: &[u8] = &[0b01100011, 0b11011011, 0b10101111];
        let mut reader = BitReader::new(data);
        assert_eq!(reader.read_bits(3)?, BitSequence::new(0b011, 3));
        assert_eq!(reader.borrow_reader_from_boundary().read_u8()?, 0b11011011);
        assert_eq!(reader.read_bits(8)?, BitSequence::new(0b10101111, 8));
        Ok(())
    }
}
