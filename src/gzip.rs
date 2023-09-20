#![forbid(unsafe_code)]

use std::io::{BufRead, Write};

use anyhow::{ensure, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use crc::Crc;
use log::*;

use crate::{
    bit_reader::BitReader,
    deflate::DeflateReader,
    //tracking_writer::TrackingWriter,
};

////////////////////////////////////////////////////////////////////////////////

const ID1: u8 = 0x1f;
const ID2: u8 = 0x8b;

const CM_DEFLATE: u8 = 8;

const FTEXT_OFFSET: u8 = 0;
const FHCRC_OFFSET: u8 = 1;
const FEXTRA_OFFSET: u8 = 2;
const FNAME_OFFSET: u8 = 3;
const FCOMMENT_OFFSET: u8 = 4;

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug, Default)]
pub struct MemberHeader {
    pub compression_method: CompressionMethod,
    pub modification_time: u32,
    pub extra: Option<Vec<u8>>,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub extra_flags: u8,
    pub os: u8,
    pub has_crc: bool,
    pub is_text: bool,
}

impl MemberHeader {
    pub fn crc16(&self) -> u16 {
        let crc = Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);
        let mut digest = crc.digest();

        digest.update(&[ID1, ID2, self.compression_method.into(), self.flags().0]);
        digest.update(&self.modification_time.to_le_bytes());
        digest.update(&[self.extra_flags, self.os]);

        if let Some(extra) = &self.extra {
            digest.update(&(extra.len() as u16).to_le_bytes());
            digest.update(extra);
        }

        if let Some(name) = &self.name {
            digest.update(name.as_bytes());
            digest.update(&[0]);
        }

        if let Some(comment) = &self.comment {
            digest.update(comment.as_bytes());
            digest.update(&[0]);
        }

        (digest.finalize() & 0xffff) as u16
    }

    pub fn flags(&self) -> MemberFlags {
        let mut flags = MemberFlags(0);
        flags.set_is_text(self.is_text);
        flags.set_has_crc(self.has_crc);
        flags.set_has_extra(self.extra.is_some());
        flags.set_has_name(self.name.is_some());
        flags.set_has_comment(self.comment.is_some());
        flags
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompressionMethod {
    Deflate,
    Unknown(u8),
}

impl From<u8> for CompressionMethod {
    fn from(value: u8) -> Self {
        match value {
            CM_DEFLATE => Self::Deflate,
            x => Self::Unknown(x),
        }
    }
}

impl From<CompressionMethod> for u8 {
    fn from(method: CompressionMethod) -> u8 {
        match method {
            CompressionMethod::Deflate => CM_DEFLATE,
            CompressionMethod::Unknown(x) => x,
        }
    }
}

impl Default for CompressionMethod {
    fn default() -> Self {
        Self::Unknown(42)
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
pub struct MemberFlags(u8);

#[allow(unused)]
impl MemberFlags {
    fn bit(&self, n: u8) -> bool {
        (self.0 >> n) & 1 != 0
    }

    fn set_bit(&mut self, n: u8, value: bool) {
        if value {
            self.0 |= 1 << n;
        } else {
            self.0 &= !(1 << n);
        }
    }

    pub fn is_text(&self) -> bool {
        self.bit(FTEXT_OFFSET)
    }

    pub fn set_is_text(&mut self, value: bool) {
        self.set_bit(FTEXT_OFFSET, value)
    }

    pub fn has_crc(&self) -> bool {
        self.bit(FHCRC_OFFSET)
    }

    pub fn set_has_crc(&mut self, value: bool) {
        self.set_bit(FHCRC_OFFSET, value)
    }

    pub fn has_extra(&self) -> bool {
        self.bit(FEXTRA_OFFSET)
    }

    pub fn set_has_extra(&mut self, value: bool) {
        self.set_bit(FEXTRA_OFFSET, value)
    }

    pub fn has_name(&self) -> bool {
        self.bit(FNAME_OFFSET)
    }

    pub fn set_has_name(&mut self, value: bool) {
        self.set_bit(FNAME_OFFSET, value)
    }

    pub fn has_comment(&self) -> bool {
        self.bit(FCOMMENT_OFFSET)
    }

    pub fn set_has_comment(&mut self, value: bool) {
        self.set_bit(FCOMMENT_OFFSET, value)
    }
}

////////////////////////////////////////////////////////////////////////////////

#[derive(Debug)]
pub struct MemberFooter {
    pub data_crc32: u32,
    pub data_size: u32,
}

////////////////////////////////////////////////////////////////////////////////

pub struct GzipReader<T> {
    reader: T,
}

impl<T: BufRead> GzipReader<T> {
    pub fn new(reader: T) -> Self {
        Self { reader }
    }

    pub fn decompress<W: Write>(mut self, output: W) -> Result<(T, W)> {
        info!("parsing gzip header");
        let (_header, _flags) = Self::parse_header(&mut self.reader)?;

        info!("parsing deflate format");
        let mut deflate_reader = DeflateReader::new(BitReader::new(&mut self.reader));
        let (actual_size, (actual_crc, writer)) = deflate_reader.deflate(output)?;
        let data_crc32 = self.reader.read_u32::<LittleEndian>()?;
        let data_size = self.reader.read_u32::<LittleEndian>()?;
        ensure!(data_size == actual_size, "length check failed");
        ensure!(data_crc32 == actual_crc, "crc32 check failed");
        Ok((self.reader, writer))
    }

    fn parse_header(header: &mut T) -> Result<(MemberHeader, MemberFlags)> {
        let id_1 = header.read_u8()?;
        ensure!(id_1 == ID1, "wrong id values");

        let id_2 = header.read_u8()?;
        ensure!(id_2 == ID2, "wrong id values");

        let mut pheader = MemberHeader {
            compression_method: header.read_u8()?.into(),
            ..Default::default()
        };
        debug!("CM:\t{:?}", pheader.compression_method);
        ensure!(
            pheader.compression_method == CompressionMethod::Deflate,
            "unsupported compression method"
        );

        let pflags = MemberFlags(header.read_u8()?);
        debug!("FLG:\t{:#010b}", pflags.0);

        pheader.modification_time = header.read_u32::<LittleEndian>()?;
        pheader.extra_flags = header.read_u8()?;
        pheader.os = header.read_u8()?;
        debug!("MTIME:\t{}", pheader.modification_time);
        debug!("XFL:\t{}", pheader.extra_flags);
        debug!("OS:\t{}", pheader.os);

        if pflags.has_extra() {
            let len: usize = header.read_u16::<LittleEndian>()?.into();
            let mut extra = vec![0; len];
            header.read_exact(&mut extra)?;
            pheader.extra = Some(extra);
            debug!(
                "EXTRA:\t{:?}",
                String::from_utf8(pheader.extra.clone().unwrap())
            );
        }

        if pflags.has_name() {
            let mut name = vec![];
            header.read_until(0, &mut name)?;
            pheader.name = Some(String::from_utf8(name)?);
            debug!("NAME:\t{:?}", pheader.name);
        }

        if pflags.has_comment() {
            let mut comment = vec![];
            header.read_until(0, &mut comment)?;
            pheader.comment = Some(String::from_utf8(comment)?);
            debug!("COMMENT:\t{:?}", pheader.comment);
        }

        if pflags.is_text() {
            pheader.is_text = true;
            debug!("IS_TEXT:\ttrue");
        }

        if pflags.has_crc() {
            let crc = header.read_u16::<LittleEndian>()?;
            debug!("CRC:\t{:#b}", crc);

            /* Caveat: must be set before calculating crc16 of header. */
            pheader.has_crc = true;
            ensure!(crc == pheader.crc16(), "header crc16 check failed");
        }

        Ok((pheader, pflags))
    }
}
