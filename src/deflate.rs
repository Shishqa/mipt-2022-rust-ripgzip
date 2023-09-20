#![forbid(unsafe_code)]

use std::io::{BufRead, Write};

use anyhow::{anyhow, bail, ensure, Result};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use log::*;

use crate::bit_reader::BitReader;
use crate::huffman_coding::{self, LitLenToken};
use crate::tracking_writer::TrackingWriter;

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Default)]
pub struct BlockHeader {
    pub is_final: bool,
    pub compression_type: CompressionType,
}

#[derive(Debug, PartialEq, PartialOrd)]
pub enum CompressionType {
    Uncompressed = 0,
    FixedTree = 1,
    DynamicTree = 2,
    Reserved = 3,
}

impl Default for CompressionType {
    fn default() -> Self {
        Self::Uncompressed
    }
}

impl From<u16> for CompressionType {
    fn from(num: u16) -> Self {
        match num {
            0 => CompressionType::Uncompressed,
            1 => CompressionType::FixedTree,
            2 => CompressionType::DynamicTree,
            _ => CompressionType::Reserved,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////

pub struct DeflateReader<T> {
    bit_reader: BitReader<T>,
    reached_last: bool,
}

impl<T: BufRead> DeflateReader<T> {
    pub fn new(bit_reader: BitReader<T>) -> Self {
        Self {
            bit_reader,
            reached_last: false,
        }
    }

    pub fn next_block(&mut self) -> Option<Result<(BlockHeader, &mut BitReader<T>)>> {
        if self.reached_last {
            return None;
        }
        let mut header = BlockHeader::default();
        match self.bit_reader.read_bits(1) {
            Ok(is_final) => {
                header.is_final = is_final.bits() == 1;
                self.reached_last |= header.is_final
            }
            Err(err) => return Some(Err(anyhow!(err))),
        }
        match self.bit_reader.read_bits(2) {
            Ok(comp_type) => {
                header.compression_type = comp_type.bits().into();
            }
            Err(err) => return Some(Err(anyhow!(err))),
        }
        Some(Ok((header, &mut self.bit_reader)))
    }

    pub fn deflate<W: Write>(&mut self, output: W) -> Result<(u32, (u32, W))> {
        let mut writer = TrackingWriter::<W>::new(output);

        while let Some(result) = self.next_block() {
            match result {
                Ok((block_header, bit_reader)) => {
                    info!("processing block");
                    debug!("ISFINAL:\t{:?}", block_header.is_final);
                    debug!("BTYPE:\t{:?}", block_header.compression_type);
                    if block_header.compression_type == CompressionType::Reserved {
                        bail!("unsupported block type");
                    }

                    if block_header.compression_type == CompressionType::Uncompressed {
                        let reader = bit_reader.borrow_reader_from_boundary();
                        let len = reader.read_u16::<LittleEndian>()?;
                        let nlen = reader.read_u16::<LittleEndian>()?;
                        ensure!(len == !nlen, "nlen check failed");
                        debug!("copying {} bytes", len);
                        let mut buffer = vec![0; len.into()];
                        reader.read_exact(&mut buffer)?;
                        writer.write_all(&buffer)?;
                        continue;
                    }

                    info!("decoding trees");
                    let (litlen, dist) = match block_header.compression_type {
                        CompressionType::DynamicTree => {
                            huffman_coding::decode_litlen_distance_trees(bit_reader)?
                        }
                        CompressionType::FixedTree => huffman_coding::get_fixed_coding()?,
                        _ => bail!("bad compression type"),
                    };

                    info!("processing symbols");
                    loop {
                        let symbol = litlen.read_symbol(bit_reader)?;
                        debug!("symbol: {:?}", symbol);
                        match symbol {
                            LitLenToken::Literal(lit) => writer.write_u8(lit)?,
                            LitLenToken::Length { base, extra_bits } => {
                                let extra_len = if extra_bits != 0 {
                                    bit_reader.read_bits(extra_bits)?.bits()
                                } else {
                                    0
                                };
                                let actual_len: usize = (base + extra_len).into();

                                let dist = dist.read_symbol(bit_reader)?;
                                let extra_dist = if dist.extra_bits != 0 {
                                    bit_reader.read_bits(dist.extra_bits)?.bits()
                                } else {
                                    0
                                };
                                let actual_dist: usize = (dist.base + extra_dist).into();

                                debug!("dist: {}, len: {}", actual_dist, actual_len);

                                writer.write_previous(actual_dist, actual_len)?;
                            }
                            LitLenToken::EndOfBlock => {
                                info!("reached end of block");
                                break;
                            }
                        }
                    }
                }
                Err(err) => bail!(err),
            }
        }

        writer.flush()?;

        Ok((writer.byte_count().try_into()?, writer.crc32()))
    }
}
