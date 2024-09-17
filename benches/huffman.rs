#![feature(test)]

#[cfg(test)]
mod tests {
    extern crate test;

    use oozextract::huffman::*;
    use std::ops::BitXor;

    #[bench]
    fn naive_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| reverse_naive(&input));
    }

    #[cfg(feature = "nightly")]
    #[bench]
    fn simd_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| reverse_simd(&input));
    }

    #[bench]
    fn sse_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| reverse_sse(&input));
    }
}
