pub(crate) mod error;
pub(crate) mod input;

use crate::algorithm::Mermaid;
use crate::algorithm::{Algorithm, Leviathan};
use crate::algorithm::{Bitknit, BitknitState, Kraken};
use crate::algorithm::{Lzna, LznaState};
use crate::decoder::Core;
use crate::ooz::error::End::{Idx, Len};
use crate::ooz::error::{ErrorContext, OozError, Res, ResultBuilder, WithContext};
use crate::ooz::input::{Input, Slice};
use futures::FutureExt;
use std::io::Read;
#[cfg(feature = "tokio")]
use tokio::io::AsyncRead;

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

pub(crate) const SMALL_BLOCK: usize = 0x4000;
pub(crate) const LARGE_BLOCK: usize = 0x40000;

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

/// Decompresses Oodle data to a buffer. Methods are provided for various input types,
/// depending on crate features.
#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
pub struct Extractor {
    pos: usize,
    header: BlockHeader,
    bitknit_state: Option<BitknitState>,
    lzna_state: Option<LznaState>,
    scratch: Box<[u8; LARGE_BLOCK]>,
    tmp: Box<[u8; LARGE_BLOCK]>,
    buf: bytes::BytesMut,
}

#[cfg(feature = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl Extractor {
    pub fn extract(
        &mut self,
        input: Vec<u8>,
        output_size: usize,
    ) -> Result<Vec<u8>, wasm_bindgen::JsError> {
        let mut output = vec![0; output_size];
        self.read_from_slice(input.as_ref(), output.as_mut())?;
        Ok(output)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen::prelude::wasm_bindgen)]
impl Extractor {
    pub fn new() -> Extractor {
        Extractor {
            pos: 0,
            header: Default::default(),
            bitknit_state: None,
            lzna_state: None,
            scratch: Box::new([0; LARGE_BLOCK]),
            tmp: Box::new([0; LARGE_BLOCK]),
            buf: bytes::BytesMut::zeroed(LARGE_BLOCK),
        }
    }
}

impl Extractor {
    /// Extracts from an instance of [`std::io::Read`]
    ///
    /// Input is assumed to be buffered; wrapping unbuffered input with [`std::io::BufReader`] may improve performance
    ///
    /// `output` should be exactly large enough to hold the uncompressed data
    pub fn read<In: Read>(&mut self, input: &mut In, output: &mut [u8]) -> Result<usize, OozError> {
        self.read_sync(input, output)
    }

    /// Extracts from a byte slice
    ///
    /// `output` should be exactly large enough to hold the uncompressed data
    pub fn read_from_slice(&mut self, input: &[u8], output: &mut [u8]) -> Result<usize, OozError> {
        self.read_sync(&mut Slice { buf: input, pos: 0 }, output)
    }

    /// Extracts from an instance of [`tokio::io::AsyncRead`]
    ///
    /// Input is assumed to be buffered; wrapping unbuffered input with [`tokio::io::BufReader`] may improve performance
    ///
    /// `output` should be exactly large enough to hold the uncompressed data
    #[cfg(feature = "tokio")]
    pub async fn async_read<In: AsyncRead + Unpin>(
        &mut self,
        input: &mut In,
        output: &mut [u8],
    ) -> Result<usize, OozError> {
        self.read_async(&mut input::Async(input), output).await
    }

    /// Extracts from an instance of [`futures::stream::Stream`]
    ///
    /// Bytes in `current` will be prepended to the stream; the Option<Bytes> returned by this method
    /// should be passed in to the next `read_from_stream` call when extracting multiple compressed
    /// blocks from a stream.
    ///
    /// `output` should be exactly large enough to hold the uncompressed data
    #[cfg(feature = "async")]
    pub async fn read_from_stream<
        E: 'static + std::error::Error + Send + Sync,
        In: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    >(
        &mut self,
        stream: &mut In,
        current: Option<bytes::Bytes>,
        output: &mut [u8],
    ) -> Result<(usize, Option<bytes::Bytes>), OozError> {
        let mut input = input::ByteStream { stream, current };
        let n = self.read_async(&mut input, output).await?;
        Ok((n, input.current))
    }
}

impl Extractor {
    fn read_sync<R: AsRef<[u8]>, In: Input<R>>(
        &mut self,
        input: &mut In,
        output: &mut [u8],
    ) -> Result<usize, OozError> {
        log::debug!("reading to buf with size {}", output.len());
        let mut bytes_written = 0;
        while bytes_written < output.len() {
            if (bytes_written & 0x3FFFF) == 0 {
                self.parse_header(input)
                    .now_or_never()
                    .expect("Read is not async")?
            }
            log::debug!("Parsed header {:?}", self.header);
            match self
                .extract_block(input, output, bytes_written)
                .now_or_never()
                .expect("Read is not async")?
            {
                0 => break,
                count => {
                    bytes_written += count;
                }
            }
        }
        log::debug!("Output filled. Wrote {} bytes", bytes_written);
        Ok(bytes_written)
    }

    #[cfg(feature = "async")]
    async fn read_async<R: AsRef<[u8]>, In: Input<R>>(
        &mut self,
        input: &mut In,
        output: &mut [u8],
    ) -> Result<usize, OozError> {
        log::debug!("reading to buf with size {}", output.len());
        let mut bytes_written = 0;
        while bytes_written < output.len() {
            if (bytes_written & 0x3FFFF) == 0 {
                self.parse_header(input).await?
            }
            log::debug!("Parsed header {:?}", self.header);
            match self.extract_block(input, output, bytes_written).await? {
                0 => break,
                count => {
                    bytes_written += count;
                }
            }
        }
        log::debug!("Output filled. Wrote {} bytes", bytes_written);
        Ok(bytes_written)
    }

    async fn extract_block<S: AsRef<[u8]>, In: Input<S>>(
        &mut self,
        input: &mut In,
        output: &mut [u8],
        offset: usize,
    ) -> Res<usize> {
        let dst_bytes_left = std::cmp::min(output.len() - offset, self.header.block_size());

        if self.header.uncompressed {
            let out = self.slice_mut(output, offset, Len(dst_bytes_left))?;
            input.read_to(out).await.at(self)?;
            self.pos += dst_bytes_left;
            return Ok(out.len());
        }

        let quantum = self.parse_quantum_header(input).await?;
        log::debug!("Parsed quantum {:?}", quantum);
        match quantum {
            QuantumHeader::Compressed {
                compressed_size, ..
            } => {
                let slice = input.read_slice(&mut self.buf, compressed_size).await?;
                let input = slice.as_ref();
                if self.header.use_checksums {
                    // If you can find a file with checksums enabled maybe you can figure out which algorithm to use here
                }
                let bytes_read = match self.header.decoder_type {
                    DecoderType::Kraken => {
                        self.decode_quantum(input, output, offset, dst_bytes_left, Kraken)
                    }
                    DecoderType::Mermaid => {
                        self.decode_quantum(input, output, offset, dst_bytes_left, Mermaid)
                    }
                    DecoderType::Leviathan => {
                        self.decode_quantum(input, output, offset, dst_bytes_left, Leviathan)
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
                input.read_to(out).await.at(self)?;
                self.pos += dst_bytes_left;
                Ok(dst_bytes_left)
            }
        }
    }

    async fn parse_header<S: AsRef<[u8]>, In: Input<S>>(&mut self, input: &mut In) -> Res<()> {
        let [b1, b2] = self.read_bytes(input, 2).await.at(self)?;
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

    async fn parse_quantum_header<S: AsRef<[u8]>, In: Input<S>>(
        &mut self,
        input: &mut In,
    ) -> Res<QuantumHeader> {
        if self.header.block_size() == LARGE_BLOCK {
            let v = usize::from_be_bytes(self.read_bytes(input, 3).await?);
            let size = v & 0x3FFFF;
            if size != 0x3FFFF {
                Ok(QuantumHeader::Compressed {
                    compressed_size: size + 1,
                    // flag1: ((v >> 18) & 1) == 1,
                    // flag2: ((v >> 19) & 1) == 1,
                    // checksum: if self.header.use_checksums {
                    //     u32::from_be_bytes(self.read_bytes(input, 3).await?)
                    // } else {
                    //     0
                    // },
                })
            } else if (v >> 18) == 1 {
                let [value] = self.read_bytes(input, 1).await?;
                Ok(QuantumHeader::Memset { value })
            } else {
                self.raise(format!("Invalid header data {}", v))?
            }
        } else {
            let v = u16::from_be_bytes(self.read_bytes(input, 2).await?);
            let size = v & 0x3FFF;
            if size != 0x3FFF {
                Ok(QuantumHeader::Compressed {
                    compressed_size: usize::from(size + 1),
                    // flag1: (v >> 14) & 1 == 1,
                    // flag2: (v >> 15) & 1 == 1,
                    // checksum: if self.header.use_checksums {
                    //     u32::from_be_bytes(self.read_bytes(input, 3).await?)
                    // } else {
                    //     0
                    // },
                })
            } else {
                match v >> 14 {
                    0 => Ok(QuantumHeader::WholeMatch {
                        whole_match_distance: self.parse_whole_match(input).await?,
                    }),
                    1 => {
                        let [value] = self.read_bytes(input, 1).await?;
                        Ok(QuantumHeader::Memset { value })
                    }
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

    async fn parse_whole_match<S: AsRef<[u8]>, In: Input<S>>(
        &mut self,
        input: &mut In,
    ) -> Res<usize> {
        let v = usize::from_be_bytes(self.read_bytes(input, 2).await?);
        if v < 0x8000 {
            let mut x = 0;
            let mut pos = 0u32;
            while let Ok([b]) = self.read_bytes(input, 1).await {
                if b & 0x80 == 0 {
                    x += (b as usize + 0x80) << pos;
                    pos += 7;
                } else {
                    x += (b as usize - 0x80) << pos;
                    return Ok(v + 0x8000 + (x << 15) + 1);
                }
            }
            self.raise(format!("{}, {}, {}", v, x, pos))?
        } else {
            Ok(v - 0x8000 + 1)
        }
    }

    async fn read_bytes<const N: usize, S: AsRef<[u8]>, In: Input<S>>(
        &mut self,
        input: &mut In,
        to_read: usize,
    ) -> Res<[u8; N]> {
        self.assert_le(to_read, N)?;
        self.pos += to_read;
        input.read_array(to_read).await
    }

    fn decode_quantum<T: Algorithm>(
        &mut self,
        input: &[u8],
        output: &mut [u8],
        offset: usize,
        dst_bytes_left: usize,
        algorithm: T,
    ) -> Res<usize> {
        Core::new(
            input,
            output,
            self.scratch.as_mut(),
            self.tmp.as_mut(),
            offset,
            dst_bytes_left,
        )
        .decode_quantum(algorithm)
    }
}

impl ErrorContext for Extractor {
    fn describe(&self) -> Option<String> {
        Some(format!(
            "block header: {:?}, input bytes read: {}",
            self.header, self.pos
        ))
    }
}
