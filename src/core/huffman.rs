use crate::core::error::{ErrorContext, Res, WithContext};
use crate::core::pointer::Pointer;
use crate::core::Core;
use wide::u8x16;

pub const BASE_PREFIX: [usize; 12] = [
    0x0, 0x0, 0x2, 0x6, 0xE, 0x1E, 0x3E, 0x7E, 0xFE, 0x1FE, 0x2FE, 0x3FE,
];

#[derive(Default)]
pub struct HuffReader {
    // Array to hold the output of the huffman read array operation
    pub output: Pointer,
    pub output_end: Pointer,
    // We decode three parallel streams, two forwards, |src| and |src_mid|
    // while |src_end| is decoded backwards.
    pub src: Pointer,
    pub src_mid: Pointer,
    pub src_end: Pointer,
    pub src_mid_org: Pointer,
    pub src_bitpos: i32,
    pub src_mid_bitpos: i32,
    pub src_end_bitpos: i32,
    pub src_bits: u32,
    pub src_mid_bits: u32,
    pub src_end_bits: u32,
}

impl ErrorContext for HuffReader {}

impl HuffReader {
    pub fn decode_bytes(&mut self, core: &mut Core, lut: &HuffRevLut) -> Res<()> {
        let mut src = self.src;
        let mut src_bits = self.src_bits;
        let mut src_bitpos = self.src_bitpos;

        let mut src_mid = self.src_mid;
        let mut src_mid_bits = self.src_mid_bits;
        let mut src_mid_bitpos = self.src_mid_bitpos;

        let mut src_end = self.src_end;
        let mut src_end_bits = self.src_end_bits;
        let mut src_end_bitpos = self.src_end_bitpos;

        let mut k: usize;
        let mut n;

        let mut dst = self.output;
        let mut dst_end = self.output_end;

        assert!(src <= src_mid, "{:?} > {:?}", src, src_mid);

        if (self.src_end - src_mid)? >= 4 && (dst_end - dst)? >= 6 {
            dst_end -= 5;
            src_end -= 4;

            while dst < dst_end && src <= src_mid && src_mid <= src_end {
                src_bits |= (core.get_le_bytes(src, 4).at(core)? as u32) << src_bitpos;
                src += (31 - src_bitpos) >> 3;

                src_end_bits |= (core.get_be_bytes(src_end, 4).at(core)? as u32) << src_end_bitpos;
                src_end -= (31 - src_end_bitpos) >> 3;

                src_mid_bits |= (core.get_le_bytes(src_mid, 4).at(core)? as u32) << src_mid_bitpos;
                src_mid += (31 - src_mid_bitpos) >> 3;

                src_bitpos |= 0x18;
                src_end_bitpos |= 0x18;
                src_mid_bitpos |= 0x18;

                k = (src_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_bits >>= n as u32;
                src_bitpos -= n as i32;
                core.set(dst + 0, lut.bits2sym[k]).at(self)?;

                k = (src_end_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_end_bits >>= n as u32;
                src_end_bitpos -= n as i32;
                core.set(dst + 1, lut.bits2sym[k]).at(self)?;

                k = (src_mid_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_mid_bits >>= n as u32;
                src_mid_bitpos -= n as i32;
                core.set(dst + 2, lut.bits2sym[k]).at(self)?;

                k = (src_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_bits >>= n as u32;
                src_bitpos -= n as i32;
                core.set(dst + 3, lut.bits2sym[k]).at(self)?;

                k = (src_end_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_end_bits >>= n as u32;
                src_end_bitpos -= n as i32;
                core.set(dst + 4, lut.bits2sym[k]).at(self)?;

                k = (src_mid_bits & 0x7FF) as _;
                n = lut.bits2len[k];
                src_mid_bits >>= n as u32;
                src_mid_bitpos -= n as i32;
                core.set(dst + 5, lut.bits2sym[k]).at(self)?;
                dst += 6;
            }
            dst_end += 5;

            src -= src_bitpos >> 3;
            src_bitpos &= 7;

            src_end += 4 + (src_end_bitpos >> 3);
            src_end_bitpos &= 7;

            src_mid -= src_mid_bitpos >> 3;
            src_mid_bitpos &= 7;
        }
        while dst < dst_end {
            if (src_mid - src)? <= 1 {
                if (src_mid - src)? == 1 {
                    // no test coverage
                    src_bits |= (core.get_byte(src).at(self)? as u32) << src_bitpos;
                }
            } else {
                src_bits |= (core.get_le_bytes(src, 2).at(core)? as u32) << src_bitpos;
            }
            k = (src_bits & 0x7FF) as _;
            n = lut.bits2len[k];
            src_bitpos -= n as i32;
            src_bits >>= n as u32;
            core.set(dst, lut.bits2sym[k]).at(self)?;
            dst += 1;
            src += (7 - src_bitpos) >> 3;
            src_bitpos &= 7;

            if dst < dst_end {
                if (src_end - src_mid)? <= 1 {
                    if (src_end - src_mid)? == 1 {
                        let mid = core.get_byte(src_mid).at(self)? as u32;
                        src_end_bits |= mid << src_end_bitpos;
                        src_mid_bits |= mid << src_mid_bitpos;
                    }
                } else {
                    let v = core.get_le_bytes((src_end - 2)?, 2).at(self)? as u32;
                    src_end_bits |= (((v >> 8) | (v << 8)) & 0xffff) << src_end_bitpos;
                    src_mid_bits |=
                        (core.get_le_bytes(src_mid, 2).at(self)? as u32) << src_mid_bitpos;
                }
                n = lut.bits2len[(src_end_bits & 0x7FF) as usize];
                core.set(dst, lut.bits2sym[(src_end_bits & 0x7FF) as usize])
                    .at(self)?;
                dst += 1;
                src_end_bitpos -= n as i32;
                src_end_bits >>= n as u32;
                src_end -= (7 - src_end_bitpos) >> 3;
                src_end_bitpos &= 7;
                if dst < dst_end {
                    n = lut.bits2len[(src_mid_bits & 0x7FF) as usize];
                    core.set(dst, lut.bits2sym[(src_mid_bits & 0x7FF) as usize])
                        .at(self)?;
                    dst += 1;
                    src_mid_bitpos -= n as i32;
                    src_mid_bits >>= n as u32;
                    src_mid += (7 - src_mid_bitpos) >> 3;
                    src_mid_bitpos &= 7;
                }
            }
            assert!(src <= src_mid, "{:?} > {:?}", src, src_mid);
            assert!(src_mid <= src_end, "{:?} > {:?}", src_mid, src_end);
        }
        self.assert_eq(src, self.src_mid_org)?;
        self.assert_eq(src_end, src_mid)?;
        Ok(())
    }
}

pub struct HuffRange {
    pub symbol: u16,
    pub num: u16,
}

pub struct HuffRevLut {
    // Mapping that maps a bit pattern to a code length.
    pub bits2len: [u8; 2048],
    // Mapping that maps a bit pattern to a symbol.
    pub bits2sym: [u8; 2048],
}

impl Core<'_> {
    pub fn make_lut(&mut self, prefix_cur: &[usize; 12], syms: &[u8; 1280]) -> Res<HuffRevLut> {
        let mut bits2len = [0u8; 2048 + 16];
        let mut bits2sym = [0u8; 2048 + 16];
        let mut currslot = 0;
        for i in 1..11u8 {
            let start = BASE_PREFIX[usize::from(i)];
            let count = prefix_cur[usize::from(i)] - start;
            if count != 0 {
                let stepsize = 1 << (11 - i);
                let num_to_set = count << (11 - i);
                assert!(currslot + num_to_set <= 2048);
                bits2len[currslot..][..num_to_set].fill(i);

                for j in 0..count {
                    let dst = currslot + stepsize * j;
                    bits2sym[dst..][..stepsize].fill(syms[start + j])
                }
                currslot += num_to_set;
            }
        }
        if prefix_cur[11] - BASE_PREFIX[11] != 0 {
            let num_to_set = prefix_cur[11] - BASE_PREFIX[11];
            assert!(currslot + num_to_set <= 2048);
            bits2len[currslot..][..num_to_set].fill(11);
            bits2sym[currslot..][..num_to_set]
                .copy_from_slice(&syms[BASE_PREFIX[11]..][..num_to_set]);
            currslot += num_to_set;
        }

        self.assert_eq(currslot, 2048)?;
        Ok(HuffRevLut {
            bits2len: reverse_lut(&bits2len),
            bits2sym: reverse_lut(&bits2sym),
        })
    }
}

#[allow(unreachable_code)]
pub fn reverse_lut(input: &[u8; 2064]) -> [u8; 2048] {
    #[cfg(all(feature = "x86_sse", any(target_arch = "x86", target_arch = "x86_64")))]
    return reverse_sse(input);
    return reverse_simd(input);
    reverse_naive(input)
}

/// 2567.903645833333 ns/iter (+/- 149.404296875) on my machine
pub fn reverse_naive(input: &[u8; 2064]) -> [u8; 2048] {
    std::array::from_fn(|i| input[((i as u16).reverse_bits() >> 5) as usize])
}

/// 145.24246174617463 ns/iter (+/- 12.897633513351337) on my machine
#[cfg(all(feature = "x86_sse", any(target_arch = "x86", target_arch = "x86_64")))]
pub fn reverse_sse(input: &[u8; 2064]) -> [u8; 2048] {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;
    let mut result = [0; 2048];
    let mut output = &mut result[..];
    const OFFSETS: [usize; 32] = [
        0x00, 0x80, 0x40, 0xC0, 0x20, 0xA0, 0x60, 0xE0, 0x10, 0x90, 0x50, 0xD0, 0x30, 0xB0, 0x70,
        0xF0, 0x08, 0x88, 0x48, 0xC8, 0x28, 0xA8, 0x68, 0xE8, 0x18, 0x98, 0x58, 0xD8, 0x38, 0xB8,
        0x78, 0xF8,
    ];
    for j in OFFSETS {
        unsafe {
            let t0 = _mm_unpacklo_epi8(
                _mm_loadl_epi64(std::ptr::addr_of!(input[j]).cast()),
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 256]).cast()),
            );
            let t1 = _mm_unpacklo_epi8(
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 512]).cast()),
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 768]).cast()),
            );
            let t2 = _mm_unpacklo_epi8(
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 1024]).cast()),
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 1280]).cast()),
            );
            let t3 = _mm_unpacklo_epi8(
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 1536]).cast()),
                _mm_loadl_epi64(std::ptr::addr_of!(input[j + 1792]).cast()),
            );

            let s0 = _mm_unpacklo_epi8(t0, t1);
            let s1 = _mm_unpacklo_epi8(t2, t3);
            let s2 = _mm_unpackhi_epi8(t0, t1);
            let s3 = _mm_unpackhi_epi8(t2, t3);

            let t0 = _mm_unpacklo_epi8(s0, s1);
            let t1 = _mm_unpacklo_epi8(s2, s3);
            let t2 = _mm_unpackhi_epi8(s0, s1);
            let t3 = _mm_unpackhi_epi8(s2, s3);

            _mm_storel_epi64(std::ptr::addr_of_mut!(output[0]).cast(), t0);
            _mm_storeh_pd(
                std::ptr::addr_of_mut!(output[1024]).cast(),
                _mm_castsi128_pd(t0),
            );
            _mm_storel_epi64(std::ptr::addr_of_mut!(output[256]).cast(), t1);
            _mm_storeh_pd(
                std::ptr::addr_of_mut!(output[1280]).cast(),
                _mm_castsi128_pd(t1),
            );
            _mm_storel_epi64(std::ptr::addr_of_mut!(output[512]).cast(), t2);
            _mm_storeh_pd(
                std::ptr::addr_of_mut!(output[1536]).cast(),
                _mm_castsi128_pd(t2),
            );
            _mm_storel_epi64(std::ptr::addr_of_mut!(output[768]).cast(), t3);
            _mm_storeh_pd(
                std::ptr::addr_of_mut!(output[1792]).cast(),
                _mm_castsi128_pd(t3),
            );
        }
        output = &mut output[8..];
    }
    result
}

/// 176.2703125 ns/iter (+/- 30.97856249999998) on my machine
pub fn reverse_simd(input: &[u8; 2064]) -> [u8; 2048] {
    let mut result = [0; 2048];
    let mut output = &mut result[..];
    const OFFSETS: [usize; 32] = [
        0x00, 0x80, 0x40, 0xC0, 0x20, 0xA0, 0x60, 0xE0, 0x10, 0x90, 0x50, 0xD0, 0x30, 0xB0, 0x70,
        0xF0, 0x08, 0x88, 0x48, 0xC8, 0x28, 0xA8, 0x68, 0xE8, 0x18, 0x98, 0x58, 0xD8, 0x38, 0xB8,
        0x78, 0xF8,
    ];
    for offset in OFFSETS {
        let i = &input[offset..];
        let t: [u8x16; 8] = std::array::from_fn(|j| u8x16::from(&i[j * 256..][..16]));
        let mut iter = t.chunks(2).map(|c| u8x16::unpack_low(c[0], c[1]));
        let t: [_; 4] = std::array::from_fn(|_| iter.next().unwrap());
        let mut iter = t.chunks(2).map(|c| {
            [
                u8x16::unpack_low(c[0], c[1]),
                u8x16::unpack_high(c[0], c[1]),
            ]
        });
        let t: [_; 2] = std::array::from_fn(|_| iter.next().unwrap());
        let t = t
            .chunks(2)
            .map(|c| {
                (
                    u8x16::unpack_low(c[0][0], c[1][0]),
                    u8x16::unpack_low(c[0][1], c[1][1]),
                    u8x16::unpack_high(c[0][0], c[1][0]),
                    u8x16::unpack_high(c[0][1], c[1][1]),
                )
            })
            .next()
            .unwrap();
        output[..8].copy_from_slice(&t.0.as_array_ref()[..8]);
        output[1024..][..8].copy_from_slice(&t.0.as_array_ref()[8..]);
        output[256..][..8].copy_from_slice(&t.1.as_array_ref()[..8]);
        output[1280..][..8].copy_from_slice(&t.1.as_array_ref()[8..]);
        output[512..][..8].copy_from_slice(&t.2.as_array_ref()[..8]);
        output[1536..][..8].copy_from_slice(&t.2.as_array_ref()[8..]);
        output[768..][..8].copy_from_slice(&t.3.as_array_ref()[..8]);
        output[1792..][..8].copy_from_slice(&t.3.as_array_ref()[8..]);
        output = &mut output[8..];
    }
    result
}

#[cfg(test)]
mod tests {
    use std::ops::BitXor;

    use super::*;

    #[test_log::test]
    fn simd_test() {
        let input: [u8; 2064] = std::array::from_fn(|i| (i as u8).bitxor((i >> 8) as u8));
        let naive = reverse_naive(&input);
        let simd = reverse_simd(&input);
        #[cfg(all(feature = "x86_sse", any(target_arch = "x86", target_arch = "x86_64")))]
        let sse = reverse_sse(&input);
        for i in 1..2048 {
            #[cfg(all(feature = "x86_sse", any(target_arch = "x86", target_arch = "x86_64")))]
            assert_eq!(naive[i], sse[i], "{}", i);
            assert_eq!(naive[i], simd[i], "{}", i);
        }
    }
}
