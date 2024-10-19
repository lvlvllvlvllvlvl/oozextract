//#![feature(portable_simd, array_chunks)]
#![allow(clippy::too_many_arguments)]
#![warn(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]
mod algorithm;
mod core;
mod extractor;

pub use crate::extractor::Extractor;

// used by benches/huffman.rs:
//pub use crate::core::huffman::{reverse_naive, reverse_simd, reverse_sse};

#[cfg(test)]
mod tests {
    use crate::extractor::Extractor;
    use std::io::Read;
    use std::{
        fs,
        io::{Seek, SeekFrom},
        path::PathBuf,
        time,
    };

    #[test_log::test]
    fn it_works() {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("testdata");
        for path in fs::read_dir(d).unwrap() {
            let path = path.unwrap().path();
            let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
            let extension = path.extension().unwrap().to_str().unwrap().to_string();
            // if extension != "bitknit" {
            //     continue;
            // }
            let start = time::Instant::now();
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
            if let Err(e) = extractor.read(buf) {
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
        log::debug!("done");
    }
}
