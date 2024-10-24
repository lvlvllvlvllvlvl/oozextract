use crate::algorithm::Leviathan;
use crate::algorithm::Mermaid;
use crate::algorithm::{Bitknit, BitknitState, Kraken};
use crate::algorithm::{Lzna, LznaState};
use crate::core::error::End::{Idx, Len};
use crate::core::error::{ErrorContext, Res, ResultBuilder, WithContext};
use crate::core::Core;
use std::io::Read;

#[derive(Debug, Default)]
pub enum DecoderType {
    #[default]
    Lzna = 0x5,
    Kraken = 0x6,
    Mermaid = 0xA,
    Bitknit = 0xB,
    Leviathan = 0xC,
}

/// Header in front of each 256k block
#[derive(Debug, Default)]
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

pub struct Extractor<In: Read> {
    input: In,
    pos: usize,
    header: BlockHeader,
    bitknit_state: Option<BitknitState>,
    lzna_state: Option<LznaState>,
}

impl<In: Read> Extractor<In> {
    /// Buf should be the expected size of the output file.
    /// You could also try reading blocks of 0x40000 bytes at a time,
    /// but decompressors for some formats may fail if the output would be smaller
    /// than the input buffer, as decompressed size doesn't appear to be encoded
    /// in the compression format.
    pub fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        log::debug!("reading to buf with size {}", buf.len());
        let mut bytes_written = 0;
        while bytes_written < buf.len() {
            if (bytes_written & 0x3FFFF) == 0 {
                self.parse_header()?
            }
            log::debug!("Parsed header {:?}", self.header);
            match self.extract(buf, bytes_written)? {
                0 => break,
                count => {
                    bytes_written += count;
                }
            }
        }
        log::debug!("Output filled. Wrote {} bytes", bytes_written);
        Ok(bytes_written)
    }
}

impl<In: Read> Extractor<In> {
    pub fn new(input: In) -> Extractor<In> {
        Extractor {
            input,
            pos: 0,
            header: Default::default(),
            bitknit_state: None,
            lzna_state: None,
        }
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Res<()> {
        self.input
            .read_exact(buf)
            .at(self)
            .message(|_| format!("Failed to read {} bytes", buf.len()))?;
        self.pos += buf.len();
        Ok(())
    }

    fn extract(&mut self, output: &mut [u8], offset: usize) -> Res<usize> {
        let tmp = &mut [0; LARGE_BLOCK];
        let dst_bytes_left = std::cmp::min(output.len() - offset, self.header.block_size());

        if self.header.uncompressed {
            let out = self.slice_mut(output, offset, Idx(dst_bytes_left))?;
            self.read_exact(out).at(self)?;
            return Ok(out.len());
        }

        let quantum = self.parse_quantum_header()?;
        log::debug!("Parsed quantum {:?}", quantum);
        match quantum {
            QuantumHeader::Compressed {
                compressed_size, ..
            } => {
                let input = self.slice_mut(tmp, 0, Idx(compressed_size))?;
                self.read_exact(input).at(self)?;
                if self.header.use_checksums {
                    // If you can find a file with checksums enabled maybe you can figure out which algorithm to use here
                }
                let bytes_read = match self.header.decoder_type {
                    DecoderType::Kraken => {
                        Core::new(input, output, offset, dst_bytes_left).decode_quantum(Kraken)
                    }
                    DecoderType::Mermaid => {
                        Core::new(input, output, offset, dst_bytes_left).decode_quantum(Mermaid)
                    }
                    DecoderType::Leviathan => {
                        Core::new(input, output, offset, dst_bytes_left).decode_quantum(Leviathan)
                    }
                    DecoderType::Bitknit => {
                        if self.header.restart_decoder {
                            self.bitknit_state = Some(BitknitState::new());
                            self.header.restart_decoder = false;
                        }
                        let out = self.slice_mut(output, 0, Idx(offset + dst_bytes_left))?;
                        let state = self
                            .bitknit_state
                            .as_mut()
                            .msg_of(&"Bitknit uninitialized")?;
                        let mut bitknit = Bitknit::new(input, out, state, offset);
                        bitknit.decode()
                    }
                    DecoderType::Lzna => {
                        if self.header.restart_decoder {
                            self.lzna_state = Some(LznaState::new());
                            self.header.restart_decoder = false;
                        }
                        let out = self.slice_mut(output, 0, Idx(offset + dst_bytes_left))?;
                        let state = self.lzna_state.as_mut().msg_of(&"Lzna uninitialized")?;
                        Lzna::new(input, out, offset).decode_quantum(state)
                    }
                }
                .at(self)?;
                self.assert_eq(bytes_read, compressed_size)?;
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
                // no test coverage
                if whole_match_distance > offset {
                    self.raise(format!(
                        "Distance {} invalid - only {} bytes buffered",
                        whole_match_distance, offset
                    ))?
                }
                let from = offset - whole_match_distance;
                let to = from + dst_bytes_left;
                output.copy_within(from..to, offset);
                Ok(dst_bytes_left)
            }
            QuantumHeader::Memset { value } => {
                // no test coverage
                self.slice_mut(output, offset, Len(dst_bytes_left))?
                    .fill(value);
                log::debug!("Set block to {}", value);
                Ok(dst_bytes_left)
            }
            QuantumHeader::Uncompressed => {
                // no test coverage
                let out = self.slice_mut(output, offset, Len(dst_bytes_left))?;
                self.read_exact(out).at(self)?;
                Ok(dst_bytes_left)
            }
        }
    }

    fn parse_header(&mut self) -> Res<()> {
        let [b1, b2] = self.read_bytes(2).at(self)?;
        if ((b1 & 0xF) != 0xC) || (((b1 >> 4) & 3) != 0) {
            self.raise(format!("Invalid header {:X}", u16::from_le_bytes([b1, b2])))?
        } else {
            self.header = BlockHeader {
                restart_decoder: (b1 >> 7) & 1 == 1,
                uncompressed: (b1 >> 6) & 1 == 1,
                decoder_type: self.decoder_type(b2 & 0x7F).at(self)?,
                use_checksums: (b2 >> 7) != 0,
            };
            Ok(())
        }
    }

    fn parse_quantum_header(&mut self) -> Res<QuantumHeader> {
        if self.header.block_size() == LARGE_BLOCK {
            let v = usize::from_be_bytes(self.read_bytes(3)?);
            let size = v & 0x3FFFF;
            if size != 0x3ffff {
                Ok(QuantumHeader::Compressed {
                    compressed_size: size + 1,
                    flag1: ((v >> 18) & 1) == 1,
                    flag2: ((v >> 19) & 1) == 1,
                    checksum: if self.header.use_checksums {
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
                self.raise(format!("Invalid header data {}", v))?
            }
        } else {
            let v = u16::from_be_bytes(self.read_bytes(2)?);
            let size = v & 0x3FFF;
            if size != 0x3FFF {
                Ok(QuantumHeader::Compressed {
                    compressed_size: usize::from(size + 1),
                    flag1: (v >> 14) & 1 == 1,
                    flag2: (v >> 15) & 1 == 1,
                    checksum: if self.header.use_checksums {
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
                    _ => self.raise(format!("unexpected match type {}", v))?,
                }
            }
        }
    }

    fn decoder_type(&mut self, value: u8) -> Res<DecoderType> {
        match value {
            0x5 => Ok(DecoderType::Lzna),
            0x6 => Ok(DecoderType::Kraken),
            0xA => Ok(DecoderType::Mermaid),
            0xB => Ok(DecoderType::Bitknit),
            0xC => Ok(DecoderType::Leviathan),
            _ => self.raise(format!("Unknown decoder type {:X}", value))?,
        }
    }

    fn parse_whole_match(&mut self) -> Res<usize> {
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
            self.raise(format!("{}, {}, {}", v, x, pos))?
        } else {
            Ok(v - 0x8000 + 1)
        }
    }

    fn read_bytes<const N: usize>(&mut self, to_read: usize) -> Res<[u8; N]> {
        self.assert_le(to_read, N)?;
        let mut buf = [0; N];
        self.read_exact(&mut buf[N - to_read..]).at(self)?;
        Ok(buf)
    }
}

impl<In: Read> ErrorContext for Extractor<In> {
    fn describe(&self) -> Option<String> {
        Some(format!(
            "header: {:?}, input bytes read: {}",
            self.header, self.pos
        ))
    }
}
