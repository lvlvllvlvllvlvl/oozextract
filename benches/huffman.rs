#![feature(test)]

#[cfg(test)]
mod tests {
    extern crate test;

    use std::ops::BitXor;

    #[bench]
    fn naive_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| oozextract::reverse_naive(&input));
    }

    #[bench]
    fn simd_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| oozextract::reverse_simd(&input));
    }

    #[bench]
    fn sse_bench(b: &mut test::Bencher) {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        b.iter(|| oozextract::reverse_sse(&input));
    }
}
