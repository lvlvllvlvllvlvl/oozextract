//!
//! Extracts data compressed in the Kraken, Mermaid, Selkie, Leviathan, LZNA, or Bitknit formats.
//!
//! ## Features:
//! - `async`: Enables the [`Extractor::read_from_stream`] method, for runtime-agnostic extraction from bytes streams such as the one returned by `reqwest::Response::bytes_stream`.
//! - `tokio`: Enables extraction from [`tokio::io::AsyncRead`].
//! - `cli`: Builds the `unoodle` command-line executable.
#![cfg_attr(nightly, feature(doc_auto_cfg))]
#![allow(clippy::too_many_arguments)]
#![warn(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_asserts_for_indexing
)]
mod algorithm;
mod decoder;
mod ooz;

pub use crate::ooz::error::OozError;
pub use crate::ooz::Extractor;

#[cfg(feature = "x86_sse")]
pub use crate::decoder::huffman::{reverse_naive, reverse_portable, reverse_x86};

#[cfg(test)]
mod tests {
    use crate::ooz::Extractor;
    use crate::OozError;
    use bytes::Buf;
    use std::fs::File;
    #[cfg(feature = "async")]
    use std::future::Future;
    use std::io::Read;
    use std::{
        fs,
        io::{Seek, SeekFrom},
        path::PathBuf,
        time,
    };
    #[cfg(feature = "async")]
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    #[cfg(feature = "async")]
    use tokio::runtime::Runtime;
    #[cfg(feature = "async")]
    use tokio::task::JoinSet;
    #[cfg(feature = "async")]
    use tokio_util::io::ReaderStream;

    #[test_log::test]
    #[allow(clippy::unwrap_used)]
    fn read() {
        loop_files(|extractor, file, output| {
            let mut buf = [0; 8];
            file.read_exact(&mut buf)?;
            log::debug!("header {:?}", buf);
            if buf[4] == 0x8C {
                buf[4..].fill_with(Default::default);
                file.seek(SeekFrom::Start(4))?;
            }
            let len = u64::from_le_bytes(buf) as usize;
            output.resize(len, 0);
            extractor.read(file, output)?;
            Ok(())
        })
        .unwrap();
    }

    #[test_log::test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn uncompressed_block_at_nonzero_offset() {
        const BLOCK: usize = 0x40000;
        const HDR: [u8; 2] = [0x4C, 0x06];

        let mut input = Vec::with_capacity(2 * (HDR.len() + BLOCK));
        input.extend_from_slice(&HDR);
        input.extend(std::iter::repeat(0xAB).take(BLOCK));
        input.extend_from_slice(&HDR);
        input.extend(std::iter::repeat(0xCD).take(BLOCK));

        let mut output = vec![0u8; 2 * BLOCK];
        let n = Extractor::new()
            .read_from_slice(&input, &mut output)
            .unwrap();

        assert_eq!(n, 2 * BLOCK);
        assert!(output[..BLOCK].iter().all(|&b| b == 0xAB));
        assert!(output[BLOCK..].iter().all(|&b| b == 0xCD));
    }

    #[test_log::test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn read_from_slice() {
        loop_files(|extractor, file, output| {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            let mut input = buf.as_slice();
            assert!(input.len() > 8);
            log::debug!("header {:?}", &input[..8]);
            let len = if input[4] == 0x8C {
                input.get_u32_le() as usize
            } else {
                input.get_u64_le() as usize
            };
            output.resize(len, 0);
            extractor.read_from_slice(input, output)?;
            Ok(())
        })
        .unwrap();
    }

    #[cfg(feature = "tokio")]
    #[test_log::test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn read_async() {
        Runtime::new()
            .unwrap()
            .block_on(loop_files_async(Box::leak(Box::new(read_file_async))))
            .unwrap();
    }

    #[cfg(feature = "tokio")]
    async fn read_file_async(
        mut file: tokio::fs::File,
    ) -> Result<bytes::BytesMut, Box<dyn std::error::Error>> {
        let mut buf = [0; 8];
        file.read_exact(&mut buf).await?;
        log::debug!("header {:?}", buf);
        if buf[4] == 0x8C {
            buf[4..].fill_with(Default::default);
            file.seek(SeekFrom::Start(4)).await?;
        }
        let len = u64::from_le_bytes(buf) as usize;
        let mut output = bytes::BytesMut::zeroed(len);
        Extractor::new().async_read(&mut file, &mut output).await?;
        Ok(output)
    }

    #[cfg(feature = "async")]
    #[test_log::test]
    #[allow(clippy::unwrap_used, clippy::indexing_slicing)]
    fn read_stream() {
        Runtime::new()
            .unwrap()
            .block_on(loop_files_async(Box::leak(Box::new(read_file_stream))))
            .unwrap();
    }

    #[cfg(feature = "async")]
    async fn read_file_stream(
        mut file: tokio::fs::File,
    ) -> Result<bytes::BytesMut, Box<dyn std::error::Error>> {
        let mut buf = [0; 8];
        file.read_exact(&mut buf).await?;
        log::debug!("header {:?}", buf);
        if buf[4] == 0x8C {
            buf[4..].fill_with(Default::default);
            file.seek(SeekFrom::Start(4)).await?;
        }
        let len = u64::from_le_bytes(buf) as usize;
        let mut output = bytes::BytesMut::zeroed(len);
        Extractor::new()
            .read_from_stream(&mut ReaderStream::new(file), None, &mut output)
            .await?;
        Ok(output)
    }

    #[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
    fn loop_files(
        mut extract: impl FnMut(&mut Extractor, &mut File, &mut Vec<u8>) -> Result<(), OozError>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("testdata");
        let mut extractor = Extractor::new();
        let mut buf = Vec::new();
        for path in fs::read_dir(d)? {
            let path = path?.path();
            let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
            let extension = path.extension().unwrap().to_str().unwrap().to_string();
            // if extension != "bitknit" {
            //     continue;
            // }
            let mut file = File::open(path)?;
            let start = time::Instant::now();
            if let Err(e) = extract(&mut extractor, &mut file, &mut buf) {
                log::error!("Extracting {}.{} failed: {}", filename, extension, e);
                panic!();
            }
            log::info!(
                "Extracting {}.{} took {:?}",
                filename,
                extension,
                start.elapsed()
            );

            let verify_file = format!("verify/{}", filename);
            log::debug!("compare to file {}", verify_file);
            let expected = fs::read(verify_file)?;
            assert_eq!(buf.len(), expected.len());
            for (i, (actual, expect)) in buf.iter().zip(expected.iter()).enumerate() {
                assert_eq!(
                    actual, expect,
                    "difference in {}.{} at byte {}",
                    filename, extension, i
                );
            }
        }
        log::debug!("done");
        Ok(())
    }

    #[cfg(feature = "async")]
    #[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
    async fn loop_files_async<
        Fut: Send + Future<Output = Result<bytes::BytesMut, Box<dyn std::error::Error>>>,
        Fun: Send + Sync + Fn(tokio::fs::File) -> Fut,
    >(
        extract: &'static Fun,
    ) -> Result<(), OozError> {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("testdata");
        let mut tasks = JoinSet::new();
        for path in fs::read_dir(d)? {
            tasks.spawn(async {
                let path = path.unwrap().path();
                let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
                let extension = path.extension().unwrap().to_str().unwrap().to_string();
                // if extension != "bitknit" {
                //     continue;
                // }
                let file = tokio::fs::File::open(path).await.unwrap();
                let start = time::Instant::now();
                match extract(file).await {
                    Ok(buf) => {
                        log::info!(
                            "Extracting {}.{} took {:?}",
                            filename,
                            extension,
                            start.elapsed()
                        );

                        let verify_file = format!("verify/{}", filename);
                        log::debug!("compare to file {}", verify_file);
                        let expected = fs::read(verify_file).unwrap();
                        assert_eq!(buf.len(), expected.len());
                        for (i, (actual, expect)) in buf.iter().zip(expected.iter()).enumerate() {
                            assert_eq!(
                                actual, expect,
                                "difference in {}.{} at byte {}",
                                filename, extension, i
                            );
                        }
                    }
                    Err(e) => {
                        log::error!("Extracting {}.{} failed: {}", filename, extension, e);
                        panic!();
                    }
                }
            });
        }
        tasks.join_all().await;
        log::debug!("done");
        Ok(())
    }
}
