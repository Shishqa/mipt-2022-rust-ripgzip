#![forbid(unsafe_code)]

use std::{collections::HashMap, convert::TryFrom, io::BufRead};

use anyhow::{anyhow, ensure, Result};
use log::*;

use crate::bit_reader::{BitReader, BitSequence};

////////////////////////////////////////////////////////////////////////////////

pub fn decode_litlen_distance_trees<T: BufRead>(
    bit_reader: &mut BitReader<T>,
) -> Result<(HuffmanCoding<LitLenToken>, HuffmanCoding<DistanceToken>)> {
    info!("dynamic tree");

    let hlit = bit_reader.read_bits(5)?.bits() + 257;
    let hdist = bit_reader.read_bits(5)?.bits() + 1;
    let hclen = bit_reader.read_bits(4)?.bits() + 4;
    debug!("HLIT:\t{:?}", hlit);
    debug!("HDIST:\t{:?}", hdist);
    debug!("HCLEN:\t{:?}", hclen);

    static TREE_CODE_ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];

    ensure!(hclen <= 19);
    let mut tree_len = vec![0; 19];
    for i in 0..hclen {
        let len = bit_reader.read_bits(3)?;
        tree_len[TREE_CODE_ORDER[i as usize]] = len.bits().into();
    }
    let tree_code_huffman = HuffmanCoding::<TreeCodeToken>::from_lengths(&tree_len)?;

    let mut code_lengths = Vec::<usize>::with_capacity((hlit + hdist).into());
    loop {
        let code = tree_code_huffman.read_symbol(bit_reader)?;
        debug!("decode: {:?}", code);
        match code {
            TreeCodeToken::Length(some) => code_lengths.push(some.into()),
            TreeCodeToken::CopyPrev => {
                let num_repetitions = bit_reader.read_bits(2)?.bits() + 3;
                ensure!(code_lengths.last().is_some(), "nothing to copy");
                let prev_len = *code_lengths.last().unwrap();
                code_lengths.append(&mut vec![prev_len; num_repetitions.into()]);
            }
            TreeCodeToken::RepeatZero { base, extra_bits } => {
                let extra = bit_reader.read_bits(extra_bits)?;
                code_lengths.append(&mut vec![0; (base + extra.bits()).into()]);
            }
        }
        if code_lengths.len() == (hlit + hdist).into() {
            break;
        }
    }

    let (lit_lengths, dist_lengths) = code_lengths.split_at(hlit.into());

    Ok((
        HuffmanCoding::<LitLenToken>::from_lengths(lit_lengths)?,
        HuffmanCoding::<DistanceToken>::from_lengths(dist_lengths)?,
    ))
}

pub fn get_fixed_coding() -> Result<(HuffmanCoding<LitLenToken>, HuffmanCoding<DistanceToken>)> {
    info!("fixed tree");
    let mut litlen_map = HashMap::<BitSequence, LitLenToken>::with_capacity(288);
    for lit in 0..=287 {
        let code = match lit {
            0..=143 => BitSequence::new(0b00110000 + lit, 8),
            144..=255 => BitSequence::new(0b110010000 + lit - 144, 9),
            256..=279 => BitSequence::new(lit - 256, 7),
            280..=287 => BitSequence::new(0b11000000 + lit - 280, 8),
            _ => unreachable!(),
        };
        litlen_map.insert(code, HuffmanCodeWord(lit).try_into()?);
    }
    let litlen_coding = HuffmanCoding::<LitLenToken>::new(litlen_map);

    let mut dist_map = HashMap::<BitSequence, DistanceToken>::with_capacity(32);
    for lit in 0..=31 {
        let code = BitSequence::new(lit, 5);
        dist_map.insert(code, HuffmanCodeWord(lit).try_into()?);
    }
    let dist_coding = HuffmanCoding::<DistanceToken>::new(dist_map);

    Ok((litlen_coding, dist_coding))
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug)]
pub enum TreeCodeToken {
    Length(u8),
    CopyPrev,
    RepeatZero { base: u16, extra_bits: u8 },
}

impl TryFrom<HuffmanCodeWord> for TreeCodeToken {
    type Error = anyhow::Error;

    fn try_from(value: HuffmanCodeWord) -> Result<Self> {
        debug!("tree code {}", value.0);
        match value.0 {
            0..=15 => Ok(Self::Length(value.0.try_into()?)),
            16 => Ok(Self::CopyPrev),
            17 => Ok(Self::RepeatZero {
                base: 3,
                extra_bits: 3,
            }),
            18 => Ok(Self::RepeatZero {
                base: 11,
                extra_bits: 7,
            }),
            _ => Err(anyhow!("CL bad code: {}", value.0)),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug)]
pub enum LitLenToken {
    Literal(u8),
    EndOfBlock,
    Length { base: u16, extra_bits: u8 },
}

impl TryFrom<HuffmanCodeWord> for LitLenToken {
    type Error = anyhow::Error;

    fn try_from(value: HuffmanCodeWord) -> Result<Self> {
        debug!("litlen code {}", value.0);
        match value.0 {
            0..=255 => Ok(Self::Literal(value.0.try_into()?)),
            256 => Ok(Self::EndOfBlock),
            257..=264 => Ok(Self::Length {
                base: value.0 - 254,
                extra_bits: 0,
            }),
            265..=284 => {
                let extra_bits: u8 = ((value.0 - 265) / 4 + 1).try_into()?;
                let len_base = (1 << (extra_bits + 2)) + 3;
                let base = len_base + ((value.0 - 1) % 4) * (1 << extra_bits);

                Ok(Self::Length { base, extra_bits })
            }
            285 => Ok(Self::Length {
                base: 258,
                extra_bits: 0,
            }),
            _ => Err(anyhow!("LL bad code {}", value.0)),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug)]
pub struct DistanceToken {
    pub base: u16,
    pub extra_bits: u8,
}

impl TryFrom<HuffmanCodeWord> for DistanceToken {
    type Error = anyhow::Error;

    fn try_from(value: HuffmanCodeWord) -> Result<Self> {
        debug!("dist code {}", value.0);
        match value.0 {
            0..=3 => Ok(Self {
                base: value.0 + 1,
                extra_bits: 0,
            }),
            4..=29 => {
                let extra_bits: u8 = value.0 as u8 / 2 - 1;
                Ok(Self {
                    base: (1 << (extra_bits + 1)) + (value.0 % 2) * (1 << extra_bits) + 1,
                    extra_bits,
                })
            }
            _ => Err(anyhow!("D bad code: {}", value.0)),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

const MAX_BITS: usize = 15;

#[derive(Clone, Copy)]
pub struct HuffmanCodeWord(pub u16);

pub struct HuffmanCoding<T> {
    map: HashMap<BitSequence, T>,
}

impl<T> HuffmanCoding<T>
where
    T: Copy + TryFrom<HuffmanCodeWord, Error = anyhow::Error>,
{
    pub fn new(map: HashMap<BitSequence, T>) -> Self {
        Self { map }
    }

    #[allow(unused)]
    pub fn decode_symbol(&self, seq: BitSequence) -> Option<T> {
        self.map.get(&seq).copied()
    }

    pub fn read_symbol<U: BufRead>(&self, bit_reader: &mut BitReader<U>) -> Result<T> {
        let mut bits = BitSequence::new(0, 0);
        while bits.len() < 16 {
            debug!("reading huffman: {:?}", bits);
            bits = bits.concat(bit_reader.read_bits(1)?);
            if let Some(symbol) = self.decode_symbol(bits) {
                return Ok(symbol);
            }
        }
        Err(anyhow!(":("))
    }

    pub fn from_lengths(code_lengths: &[usize]) -> Result<Self> {
        info!("creating huffman coding from lengths {:#?}", code_lengths);

        let mut bl_count: [usize; MAX_BITS + 1] = [0; MAX_BITS + 1];
        for len in code_lengths {
            bl_count[*len] += 1;
        }
        bl_count[0] = 0;
        debug!("bl_count: {:#?}", bl_count);

        let mut next_code: [u16; MAX_BITS + 1] = [0; MAX_BITS + 1];
        let mut code: u16 = 0;
        for bits in 1..=MAX_BITS {
            code = (code + bl_count[bits - 1] as u16) << 1;
            next_code[bits] = code;
        }
        debug!("next_code: {:#?}", next_code);

        let mut map = HashMap::<BitSequence, T>::new();
        for (idx, len) in code_lengths.iter().enumerate() {
            if *len == 0 {
                continue;
            }
            let code = BitSequence::new(next_code[*len], *len as u8);
            map.insert(code, HuffmanCodeWord(idx as u16).try_into()?);
            debug!("new code: {} -> {:?}", idx, code);
            next_code[*len] += 1;
        }

        Ok(Self::new(map))
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Value(u16);

    impl TryFrom<HuffmanCodeWord> for Value {
        type Error = anyhow::Error;

        fn try_from(x: HuffmanCodeWord) -> Result<Self> {
            Ok(Self(x.0))
        }
    }

    #[test]
    fn from_lengths() -> Result<()> {
        let code = HuffmanCoding::<Value>::from_lengths(&[2, 3, 4, 3, 3, 4, 2])?;

        assert_eq!(
            code.decode_symbol(BitSequence::new(0b00, 2)),
            Some(Value(0)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b100, 3)),
            Some(Value(1)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b1110, 4)),
            Some(Value(2)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b101, 3)),
            Some(Value(3)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b110, 3)),
            Some(Value(4)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b1111, 4)),
            Some(Value(5)),
        );
        assert_eq!(
            code.decode_symbol(BitSequence::new(0b01, 2)),
            Some(Value(6)),
        );

        assert_eq!(code.decode_symbol(BitSequence::new(0b0, 1)), None);
        assert_eq!(code.decode_symbol(BitSequence::new(0b10, 2)), None);
        assert_eq!(code.decode_symbol(BitSequence::new(0b111, 3)), None,);

        Ok(())
    }

    #[test]
    fn read_symbol() -> Result<()> {
        let code = HuffmanCoding::<Value>::from_lengths(&[2, 3, 4, 3, 3, 4, 2])?;
        let mut data: &[u8] = &[0b10111001, 0b11001010, 0b11101101];
        let mut reader = BitReader::new(&mut data);

        assert_eq!(code.read_symbol(&mut reader)?, Value(1));
        assert_eq!(code.read_symbol(&mut reader)?, Value(2));
        assert_eq!(code.read_symbol(&mut reader)?, Value(3));
        assert_eq!(code.read_symbol(&mut reader)?, Value(6));
        assert_eq!(code.read_symbol(&mut reader)?, Value(0));
        assert_eq!(code.read_symbol(&mut reader)?, Value(2));
        assert_eq!(code.read_symbol(&mut reader)?, Value(4));
        assert!(code.read_symbol(&mut reader).is_err());

        Ok(())
    }
}
