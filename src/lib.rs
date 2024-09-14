pub mod bit_reader;
//pub mod error;
pub mod huffman;
pub mod kraken;
pub mod tans;

use std::fmt::Debug;
use std::io::{ErrorKind, Read, Seek};
use std::panic::Location;

pub use kraken::*;

#[derive(Debug)]
pub enum DecoderType {
    Lzna = 0x5,
    Kraken = 0x6,
    Mermaid = 0xA,
    Bitknit = 0xB,
    Leviathan = 0xC,
}

/// Header in front of each 256k block
#[derive(Debug)]
pub struct BlockHeader {
    /// Type of decoder used
    pub decoder_type: DecoderType,

    /// Whether to restart the decoder
    pub restart_decoder: bool,

    /// Whether this block is uncompressed
    pub uncompressed: bool,

    /// Whether this block uses checksums.
    pub use_checksums: bool,
}

const SMALL_BLOCK: usize = 0x4000;
const LARGE_BLOCK: usize = 0x40000;

impl BlockHeader {
    fn block_size(&self) -> usize {
        match self.decoder_type {
            DecoderType::Lzna => SMALL_BLOCK,
            DecoderType::Bitknit => SMALL_BLOCK,
            _ => LARGE_BLOCK,
        }
    }
}

/// Additional header in front of each large or small block ("quantum").
#[derive(Debug)]
pub enum QuantumHeader {
    Compressed {
        /// The compressed size of this quantum. If this value is 0 it means
        /// the quantum is a special quantum such as memset.
        compressed_size: usize,
        // If checksums are enabled, holds the checksum.
        checksum: u32,
        // Two flags
        flag1: bool,
        flag2: bool,
    },
    WholeMatch {
        // Whether the whole block matched a previous block
        whole_match_distance: usize,
    },
    Memset {
        value: u8,
    },
    Uncompressed,
}

pub struct Extractor<In: Read + Seek> {
    input: In,
    tmp: [u8; LARGE_BLOCK],
}

impl<In: Read + Seek> Read for Extractor<In> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        log::debug!("reading to buf with size {}", buf.len());
        let mut bytes_written = 0;
        let header = &mut [0u8, 0u8];
        while bytes_written < buf.len() {
            log::debug!(
                "read: {:?}, written: {}, buffer size: {}",
                self.input.stream_position()?,
                bytes_written,
                buf.len()
            );
            if (bytes_written & 0x3FFFF) == 0 {
                match self.input.read_exact(header) {
                    Err(ref e) if e.kind() == ErrorKind::UnexpectedEof => {
                        log::debug!("No more data. Wrote {} bytes", bytes_written);
                        return Ok(bytes_written);
                    }
                    Err(e) => return Err(e),
                    _ => (),
                }
            }
            let header = self.parse_header(header)?;
            log::debug!("Parsed header {:?}", header);
            match self.extract(buf, bytes_written, header) {
                Ok(0) => {
                    return if bytes_written > 0 {
                        log::debug!("Input empty. Wrote {} bytes", bytes_written);
                        Ok(bytes_written)
                    } else {
                        log::debug!("Write zero. Wrote {} bytes", bytes_written);
                        self.io_error(ErrorKind::WriteZero, bytes_written)
                    }
                }
                Ok(count) => {
                    bytes_written += count;
                }
                Err(e) => return Err(e),
            }
        }
        log::debug!("Output filled. Wrote {} bytes", bytes_written);
        Ok(bytes_written)
    }
}

impl<In: Read + Seek> Extractor<In> {
    pub fn new(input: In) -> Extractor<In> {
        Extractor {
            input,
            tmp: [0; LARGE_BLOCK],
        }
    }

    fn extract(
        &mut self,
        output: &mut [u8],
        offset: usize,
        header: BlockHeader,
    ) -> std::io::Result<usize> {
        let dst_bytes_left = std::cmp::min(output.len() - offset, header.block_size());

        if header.uncompressed {
            let mut bytes_copied = 0;
            while bytes_copied < dst_bytes_left {
                let count = self
                    .input
                    .read(&mut output[offset + bytes_copied..dst_bytes_left])?;
                bytes_copied += count;
                if count == 0 {
                    break;
                }
            }
            log::debug!("Copied {} bytes", bytes_copied);
            return Ok(bytes_copied);
        }

        let quantum = self.parse_quantum_header(&header)?;
        log::debug!("Parsed quantum {:?}", quantum);
        match quantum {
            QuantumHeader::Compressed {
                compressed_size, ..
            } => {
                let input = &mut self.tmp[..compressed_size];
                self.input.read_exact(input)?;
                if header.use_checksums {
                    // If you can find a file with checksums enabled maybe you can figure out which algorithm to use here
                }
                let bytes_read = match header.decoder_type {
                    DecoderType::Lzna => compressed_size,
                    DecoderType::Kraken => {
                        KrakenDecoder::new(input, output).decode_quantum(offset, dst_bytes_left)
                    }
                    DecoderType::Mermaid => compressed_size,
                    DecoderType::Bitknit => compressed_size,
                    DecoderType::Leviathan => compressed_size,
                };
                assert_eq!(bytes_read, compressed_size);
                log::debug!(
                    "Extracted {} bytes from {}",
                    dst_bytes_left,
                    compressed_size
                );
                Ok(dst_bytes_left)
            }
            QuantumHeader::WholeMatch {
                whole_match_distance,
            } => {
                if whole_match_distance > offset {
                    return self.io_error(
                        ErrorKind::InvalidInput,
                        format!(
                            "Distance {} invalid - only {} bytes buffered",
                            whole_match_distance, offset
                        ),
                    );
                }
                let from = offset - whole_match_distance;
                let to = from + dst_bytes_left;
                output.copy_within(from..to, offset);
                Ok(dst_bytes_left)
            }
            QuantumHeader::Memset { value } => {
                output[offset..][..dst_bytes_left].fill(value);
                log::debug!("Set block to {}", value);
                Ok(dst_bytes_left)
            }
            QuantumHeader::Uncompressed => self
                .input
                .read_exact(&mut output[offset..][..dst_bytes_left])
                .and(Ok(dst_bytes_left)),
        }
    }

    fn parse_header(&mut self, p: &[u8; 2]) -> Result<BlockHeader, std::io::Error> {
        let b1 = p[0];
        let b2 = p[1];
        if ((b1 & 0xF) != 0xC) || (((b1 >> 4) & 3) != 0) {
            self.io_error(ErrorKind::InvalidData, p)
        } else {
            Ok(BlockHeader {
                restart_decoder: (b1 >> 7) & 1 == 1,
                uncompressed: (b1 >> 6) & 1 == 1,
                decoder_type: self.decoder_type(b2 & 0x7F)?,
                use_checksums: (b2 >> 7) != 0,
            })
        }
    }

    fn parse_quantum_header(&mut self, header: &BlockHeader) -> std::io::Result<QuantumHeader> {
        if header.block_size() == LARGE_BLOCK {
            let v = usize::from_be_bytes(self.read_bytes(3)?);
            let size = v & 0x3FFFF;
            if size != 0x3ffff {
                Ok(QuantumHeader::Compressed {
                    compressed_size: size + 1,
                    flag1: ((v >> 18) & 1) == 1,
                    flag2: ((v >> 19) & 1) == 1,
                    checksum: if header.use_checksums {
                        u32::from_be_bytes(self.read_bytes(3)?)
                    } else {
                        0
                    },
                })
            } else if (v >> 18) == 1 {
                Ok(QuantumHeader::Memset {
                    value: self.read_bytes::<1>(1)?[0],
                })
            } else {
                self.io_error(ErrorKind::InvalidData, v)
            }
        } else {
            let v = u16::from_be_bytes(self.read_bytes(2)?);
            let size = v & 0x3FFF;
            if size != 0x3FFF {
                Ok(QuantumHeader::Compressed {
                    compressed_size: usize::from(size + 1),
                    flag1: (v >> 14) & 1 == 1,
                    flag2: (v >> 15) & 1 == 1,
                    checksum: if header.use_checksums {
                        u32::from_be_bytes(self.read_bytes(3)?)
                    } else {
                        0
                    },
                })
            } else {
                match v >> 14 {
                    0 => Ok(QuantumHeader::WholeMatch {
                        whole_match_distance: self.parse_whole_match()?,
                    }),
                    1 => Ok(QuantumHeader::Memset {
                        value: self.read_bytes::<1>(1).map(|p| p[0])?,
                    }),
                    2 => Ok(QuantumHeader::Uncompressed),
                    _ => self.io_error(ErrorKind::InvalidData, v),
                }
            }
        }
    }

    fn decoder_type(&mut self, value: u8) -> Result<DecoderType, std::io::Error> {
        match value {
            0x5 => Ok(DecoderType::Lzna),
            0x6 => Ok(DecoderType::Kraken),
            0xA => Ok(DecoderType::Mermaid),
            0xB => Ok(DecoderType::Bitknit),
            0xC => Ok(DecoderType::Leviathan),
            _ => self.io_error(ErrorKind::InvalidData, value),
        }
    }

    fn parse_whole_match(&mut self) -> std::io::Result<usize> {
        let v = usize::from(u16::from_be_bytes(self.read_bytes(2)?));
        if v < 0x8000 {
            let mut x = 0;
            let mut pos = 0u32;
            while let Ok(b) = self.read_bytes::<1>(1).map(|p| usize::from(p[0])) {
                if b & 0x80 == 0 {
                    x += (b + 0x80) << pos;
                    pos += 7;
                } else {
                    x += (b - 0x80) << pos;
                    return Ok(v + 0x8000 + (x << 15) + 1);
                }
            }
            self.io_error(ErrorKind::InvalidData, (v, x, pos))
        } else {
            Ok(v - 0x8000 + 1)
        }
    }

    fn read_bytes<const N: usize>(&mut self, to_read: usize) -> std::io::Result<[u8; N]> {
        assert!(to_read <= N);
        let mut buf = [0; N];
        if to_read < N {
            buf.fill(0)
        }
        if let Err(e) = self.input.read_exact(&mut buf[N - to_read..]) {
            log::error!(
                "{}: read failed, expected {} bytes. {:x?}",
                Location::caller(),
                to_read,
                e
            );
        }
        Ok(buf)
    }

    #[track_caller]
    fn io_error<T, D: Debug>(&mut self, kind: ErrorKind, msg: D) -> std::io::Result<T> {
        Err(std::io::Error::new(
            kind,
            format!(
                "{}: {:x?} at {}",
                Location::caller(),
                msg,
                self.input.stream_position()?
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Seek, SeekFrom},
        path::PathBuf,
    };

    #[test_log::test]
    fn it_works() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("testdata");
        for path in fs::read_dir(d).unwrap() {
            let path = path.unwrap().path();
            let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
            let extension = path.extension().unwrap().to_str().unwrap().to_string();
            log::info!("Extracting {}.{}", filename, extension);
            let mut file = fs::File::open(path).unwrap();
            let mut buf = [0; 8];
            file.read_exact(&mut buf).unwrap();
            log::debug!("header {:?}", buf);
            if buf[4] == 0x8C {
                buf[4..].fill_with(Default::default);
                file.seek(SeekFrom::Start(4)).unwrap();
            }
            let len = usize::from_le_bytes(buf);
            let buf = &mut vec![0; len];
            let mut extractor = Extractor::new(file);
            extractor.read_exact(buf).unwrap();

            if extension == "kraken" {
                let verify_file = format!("verify/{}", filename);
                log::debug!("compare to file {}", verify_file);
                let expected = std::fs::read(verify_file).unwrap();
                assert_eq!(buf.len(), expected.len());
                for (i, (l, r)) in buf.iter().zip(expected.iter()).enumerate() {
                    assert_eq!(l, r, "difference at byte {}", i);
                }
            }
        }
        log::debug!("done");
    }
}
