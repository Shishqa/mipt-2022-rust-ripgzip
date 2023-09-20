#![forbid(unsafe_code)]

use std::io::{BufRead, Write};

use anyhow::Result;

use crate::gzip::GzipReader;

mod bit_reader;
mod deflate;
mod gzip;
mod huffman_coding;
mod tracking_writer;

pub fn decompress<R: BufRead, W: Write>(mut input: R, mut output: W) -> Result<()> {
    while let Ok(buf) = input.fill_buf() {
        if buf.is_empty() {
            break;
        }
        let gz_reader = GzipReader::new(input);
        let (new_input, new_output) = gz_reader.decompress(output)?;
        input = new_input;
        output = new_output;
    }
    Ok(())
}
