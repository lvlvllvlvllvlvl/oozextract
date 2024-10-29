use bytes::Buf;
use criterion::{black_box, criterion_group, BenchmarkId, Criterion};
use oozextract::Extractor;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::ops::BitXor;
use std::path::PathBuf;

#[cfg(feature = "x86_sse")]
pub fn reverse_huff_lut(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd");
    let mut aligned_input: [u64; 258] = [0; 258];
    for (i, v) in bytemuck::cast_slice_mut(aligned_input.as_mut())
        .iter_mut()
        .enumerate()
    {
        *v = (i as u8).bitxor((i >> 8) as u8)
    }
    let input: [u8; 2064] = bytemuck::cast_slice(aligned_input.as_slice())
        .try_into()
        .unwrap();

    group.bench_function("naive", |b| {
        b.iter(|| oozextract::reverse_naive(black_box(&input)))
    });
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    group.bench_function("native", |b| {
        b.iter(|| oozextract::reverse_x86(black_box(&input)))
    });
    group.bench_function("wide", |b| {
        b.iter(|| oozextract::reverse_portable(black_box(&aligned_input)))
    });
}

pub fn extract_from_slice(c: &mut Criterion) {
    let mut extractor = Extractor::new();

    for algorithm in [
        "kraken",
        "leviathan",
        "mermaid",
        "selkie",
        "lzna",
        "bitknit",
    ] {
        let mut group = c.benchmark_group(algorithm);
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("testdata");

        for path in fs::read_dir(d).unwrap() {
            let path = path.unwrap().path();
            let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
            if path.extension().unwrap().to_str().unwrap() != algorithm {
                continue;
            }
            let mut file = File::open(path).unwrap();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            let mut input = buf.as_slice();
            assert!(input.len() > 8);
            log::debug!("header {:?}", &input[..8]);
            let len = if input[4] == 0x8C {
                input.get_u32_le() as usize
            } else {
                input.get_u64_le() as usize
            };

            group.bench_function(BenchmarkId::from_parameter(filename), |b| {
                b.iter(|| extractor.read_from_slice(black_box(input), black_box(&mut vec![0; len])))
            });
        }
        group.finish();
    }
}

#[cfg(feature = "x86_sse")]
criterion_group!(simd, reverse_huff_lut);
criterion_group!(extract, extract_from_slice);
fn main() {
    #[cfg(feature = "x86_sse")]
    simd();
    extract();
    Criterion::default().configure_from_args().final_summary();
}
