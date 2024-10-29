#[allow(unreachable_code)]
fn main() {
    #[cfg(feature = "cli")]
    {
        return cli::extract(clap::Parser::parse()).unwrap();
    }
    println!(env!("CARGO_PKG_VERSION"))
}

#[cfg(feature = "cli")]
mod cli {
    use bytes::Buf;
    use clap::Parser;
    use oozextract::Extractor;
    use std::error::Error;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::PathBuf;

    #[derive(Parser)]
    #[command(version, about, long_about = None)]
    pub struct Args {
        /// The file to extract
        file: PathBuf,

        /// (Optional) Output to FILE
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,

        /// (Optional) Verify against FILE
        #[arg(long, value_name = "FILE")]
        verify: Option<PathBuf>,

        /// Print extracted data to console (will fail if contents are not valid utf8)
        #[arg(long)]
        stdout: bool,

        /// (Optional) Output length - if not provided will read first 4 or 8 bytes of the file as output length header
        /// For files created by ooz.cpp, neither `length` nor `header` parameters need to be supplied.
        #[arg(long, value_name = "BYTES")]
        length: Option<usize>,

        /// (Optional) Header length - if not provided will use the following heuristic:
        /// 0 if `length` is provided, 4 if the fifth byte of the file is 0x8C, else 8
        /// If both `length` and `header` are provided, the first BYTES count of bytes will be skipped
        #[arg(long, value_name = "BYTES")]
        header: Option<usize>,

        /// Print progress to console
        #[arg(short, long)]
        verbose: bool,
    }

    #[cfg(feature = "cli")]
    pub fn extract(
        Args {
            file,
            length,
            header,
            output,
            verify,
            stdout,
            verbose,
        }: Args,
    ) -> Result<(), Box<dyn Error>> {
        let mut input = if let Some(len) = length {
            Vec::with_capacity(len)
        } else {
            Vec::new()
        };
        File::open(file)?.read_to_end(&mut input)?;
        let mut input = input.as_slice();

        if verbose {
            println!("Read {} bytes", input.len());
        }

        let len = if let Some(length) = length {
            if let Some(header) = header {
                input = &input[header..];
            }
            length
        } else if input[4] == 0x8C {
            if verbose {
                println!("Oodle header found at position 4, using bytes 0..3 as length header");
            }
            input.get_u32_le() as usize
        } else {
            if verbose {
                println!("Oodle header not found at position 4, Assuming 8-byte length header");
            }
            input.get_u64_le() as usize
        };

        if verbose {
            println!("Expected output: {} bytes", len);
        }

        let mut extracted = vec![0; len];
        let actual_len = Extractor::new().read_from_slice(input, &mut extracted)?;

        if verbose {
            println!("Extracted {} bytes", actual_len);
        }

        if let Some(output) = output {
            if verbose {
                if let Some(outfile) = output.as_os_str().to_str() {
                    println!("Writing to {}", outfile)
                }
            }
            File::create(output)?.write_all(&extracted)?;
        }

        if let Some(verify) = verify {
            if verbose {
                if let Some(filename) = verify.as_os_str().to_str() {
                    println!("Comparing to {}", filename)
                }
            }
            let mut verify = File::open(verify)?;
            assert_eq!(verify.metadata()?.len() as usize, extracted.len());
            let mut expect = Vec::with_capacity(extracted.len());
            verify.read_to_end(&mut expect)?;
            for (i, (actual, expected)) in extracted.iter().zip(expect.iter()).enumerate() {
                assert_eq!(actual, expected, "bad data at byte {}", i);
            }
            if verbose {
                println!("Verified {} bytes", expect.len());
            }
        }

        if stdout {
            println!("{}", std::str::from_utf8(&extracted)?);
        }
        Ok(())
    }
}
