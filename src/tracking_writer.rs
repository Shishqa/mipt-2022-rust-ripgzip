#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io::{self, Write};

use anyhow::{anyhow, ensure, Result};
use crc::{Crc, Digest};

////////////////////////////////////////////////////////////////////////////////

const HISTORY_SIZE: usize = 32768;

pub struct TrackingWriter<T> {
    inner: T,
    history: VecDeque<u8>,
    byte_count: usize,
    digest: Digest<'static, u32>,
}

impl<T: Write> Write for TrackingWriter<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written_len = self.inner.write(buf)?;
        let written = &buf[..written_len];
        self.digest.update(written);

        if written_len > HISTORY_SIZE {
            self.history.clear();
        } else if written_len + self.history.len() > HISTORY_SIZE {
            self.history
                .drain(..(written_len + self.history.len() - HISTORY_SIZE));
        }
        self.history.extend(written);
        self.byte_count += written_len;
        Ok(written_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<T: Write> TrackingWriter<T> {
    pub fn new(inner: T) -> Self {
        static CRC: Crc<u32> = Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);
        Self {
            inner,
            history: VecDeque::<u8>::with_capacity(HISTORY_SIZE),
            byte_count: 0,
            digest: CRC.digest(),
        }
    }

    /// Write a sequence of `len` bytes written `dist` bytes ago.
    pub fn write_previous(&mut self, dist: usize, len: usize) -> Result<()> {
        ensure!(dist < self.history.len(), "Trying to write very far");

        let past_begin = self.history.len() - dist;
        let past_end = if dist <= len {
            self.history.len()
        } else {
            self.history.len() - dist + len
        };

        let mut chunk: Vec<u8> = self.history.range(past_begin..past_end).copied().collect();

        let initial_len = chunk.len();
        while chunk.len() < len {
            chunk.extend_from_within(0..initial_len);
            if chunk.len() > len {
                chunk.truncate(len);
            }
        }

        match self.write(&chunk) {
            Ok(written) => {
                if written == len {
                    Ok(())
                } else {
                    Err(anyhow!("written less"))
                }
            }
            Err(msg) => Err(anyhow!(msg)),
        }
    }

    pub fn byte_count(&self) -> usize {
        self.byte_count
    }

    pub fn crc32(self) -> (u32, T) {
        (self.digest.finalize(), self.inner)
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;

    #[test]
    fn write() -> Result<()> {
        let mut buf: &mut [u8] = &mut [0u8; 10];
        let mut writer = TrackingWriter::new(&mut buf);

        assert_eq!(writer.write(&[1, 2, 3, 4])?, 4);
        assert_eq!(writer.byte_count(), 4);

        assert_eq!(writer.write(&[4, 8, 15, 16, 23])?, 5);
        assert_eq!(writer.byte_count(), 9);

        assert_eq!(writer.write(&[0, 0, 123])?, 1);
        assert_eq!(writer.byte_count(), 10);

        assert_eq!(writer.write(&[42, 124, 234, 27])?, 0);
        assert_eq!(writer.byte_count(), 10);

        let (crc, _) = writer.crc32();
        assert_eq!(crc, 2992191065);

        Ok(())
    }

    #[test]
    fn write_previous() -> Result<()> {
        let mut buf: &mut [u8] = &mut [0u8; 512];
        let mut writer = TrackingWriter::new(&mut buf);

        for i in 0..=255 {
            writer.write_u8(i)?;
        }

        writer.write_previous(192, 128)?;
        assert_eq!(writer.byte_count(), 384);

        assert!(writer.write_previous(10000, 20).is_err());
        assert_eq!(writer.byte_count(), 384);

        assert!(writer.write_previous(256, 256).is_err());
        assert_eq!(writer.byte_count(), 512);

        assert!(writer.write_previous(1, 1).is_err());
        assert_eq!(writer.byte_count(), 512);

        let (crc, _) = writer.crc32();
        assert_eq!(crc, 2733545866);

        Ok(())
    }
}
