use crate::error::{ErrorContext, OozError};
use std::array;

type LznaBitModel = u16;

/// State for a 4-bit value RANS model
struct LznaNibbleModel {
    prob: [u16; 17],
}

/// State for a 3-bit value RANS model
struct Lzna3bitModel {
    prob: [u16; 9],
}

/// State for the literal model
#[derive(Default)]
struct LznaLiteralModel {
    upper: [LznaNibbleModel; 16],
    lower: [LznaNibbleModel; 16],
    nomatch: [LznaNibbleModel; 16],
}

/// State for a model representing a far distance
struct LznaFarDistModel {
    first_lo: LznaNibbleModel,
    first_hi: LznaNibbleModel,
    second: [LznaBitModel; 31],
    third: [[LznaBitModel; 31]; 2],
}

/// State for a model representing a near distance
struct LznaNearDistModel {
    first: LznaNibbleModel,
    second: [LznaBitModel; 16],
    third: [[LznaBitModel; 16]; 2],
}

/// State for model representing the low bits of a distance
struct LznaLowBitsDistanceModel {
    d: [LznaNibbleModel; 2],
    v: LznaBitModel,
}

/// State for model used for the short lengths for recent matches
#[derive(Default)]
struct LznaShortLengthRecentModel {
    a: [Lzna3bitModel; 4],
}

/// State for model for long lengths
#[derive(Default)]
struct LznaLongLengthModel {
    first: [LznaNibbleModel; 4],
    second: LznaNibbleModel,
    third: LznaNibbleModel,
}

/// Complete LZNA state
pub struct LznaState {
    match_history: [u32; 8],
    literal: [LznaLiteralModel; 4],
    is_literal: [LznaBitModel; 12 * 8],
    typ: [LznaNibbleModel; 12 * 8],
    short_length_recent: [LznaShortLengthRecentModel; 4],
    long_length_recent: LznaLongLengthModel,
    low_bits_of_distance: [LznaLowBitsDistanceModel; 2],
    short_length: [[LznaBitModel; 4]; 12],
    near_dist: [LznaNearDistModel; 2],
    medium_length: Lzna3bitModel,
    long_length: LznaLongLengthModel,
    far_distance: LznaFarDistModel,
}

impl Default for LznaNibbleModel {
    fn default() -> Self {
        Self {
            prob: [
                0x0000, 0x0800, 0x1000, 0x1800, 0x2000, 0x2800, 0x3000, 0x3800, 0x4000, 0x4800,
                0x5000, 0x5800, 0x6000, 0x6800, 0x7000, 0x7800, 0x8000,
            ],
        }
    }
}

impl Default for Lzna3bitModel {
    fn default() -> Self {
        Self {
            prob: [
                0x0000, 0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000, 0x7000, 0x8000,
            ],
        }
    }
}

impl Default for LznaNearDistModel {
    fn default() -> Self {
        Self {
            first: Default::default(),
            second: [0x2000; 16],
            third: [[0x2000; 16]; 2],
        }
    }
}

impl Default for LznaLowBitsDistanceModel {
    fn default() -> Self {
        Self {
            v: 0x2000,
            d: Default::default(),
        }
    }
}

impl Default for LznaFarDistModel {
    fn default() -> Self {
        Self {
            first_lo: Default::default(),
            first_hi: Default::default(),
            second: [0x2000; 31],
            third: [[0x2000; 31]; 2],
        }
    }
}

impl LznaState {
    pub fn new() -> Self {
        Self {
            match_history: [1; 8],
            is_literal: [0x1000; 96],
            short_length: [[0x2000; 4]; 12],

            typ: array::from_fn(|_| Default::default()),
            literal: Default::default(),
            short_length_recent: Default::default(),
            long_length_recent: Default::default(),
            low_bits_of_distance: Default::default(),
            near_dist: Default::default(),
            medium_length: Default::default(),
            long_length: Default::default(),
            far_distance: Default::default(),
        }
    }

    fn preprocess_match_history(&mut self) {
        if self.match_history[4] >= 0xc000 {
            let mut i = 0;
            while self.match_history[4 + i] >= 0xC000 {
                i += 1;
                if i >= 4 {
                    self.match_history[7] = self.match_history[6];
                    self.match_history[6] = self.match_history[5];
                    self.match_history[5] = self.match_history[4];
                    self.match_history[4] = 4;
                    return;
                }
            }
            let t = self.match_history[i + 4];
            self.match_history[i + 4] = self.match_history[i + 3];
            self.match_history[i + 3] = self.match_history[i + 2];
            self.match_history[i + 2] = self.match_history[i + 1];
            self.match_history[4] = t;
        }
    }
}

pub struct Lzna<'a> {
    bits_a: u64,
    bits_b: u64,
    input: &'a [u8],
    output: &'a mut [u8],
    src: usize,
    dst: usize,
}

impl<'a> ErrorContext for Lzna<'a> {}

impl<'a> Lzna<'a> {
    pub(crate) fn new(input: &'a [u8], output: &'a mut [u8], dst: usize) -> Lzna<'a> {
        Self {
            input,
            output,
            dst,
            src: 0,
            bits_a: 0,
            bits_b: 0,
        }
    }

    /// Initialize bit reader with 2 parallel streams. Every decode operation
    /// swaps the two streams.
    fn init(&mut self) {
        self.bits_a = self.init_bits();
        self.bits_b = self.init_bits();
    }

    fn init_bits(&mut self) -> u64 {
        let d = self.read_byte() as i32;
        let n = d >> 4;
        assert!(n <= 8, "{}", n);
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 8) | self.read_byte() as u64;
        }
        (v << 4) | (d & 0xF) as u64
    }

    fn read_byte(&mut self) -> u8 {
        let v = self.input[self.src];
        self.src += 1;
        v
    }

    fn read(&mut self) -> u32 {
        let v = u32::from_le_bytes(*self.input[self.src..].first_chunk().unwrap());
        self.src += 4;
        v
    }

    fn write(&mut self, v: u8) {
        self.output[self.dst] = v;
        self.dst += 1;
    }

    fn copy_offset(&mut self, dist: usize, length: usize) {
        let src = self.dst - dist;
        if dist == 1 {
            let v = self.output[src];
            self.output[self.dst..][..length].fill(v);
        } else if dist > length {
            self.output.copy_within(src..src + length, self.dst);
        } else {
            for i in (0..length).step_by(dist) {
                self.output
                    .copy_within(src + i..src + length.min(dist + i), self.dst + i);
            }
        }
        self.dst += length;
    }

    /// Renormalize by filling up the RANS state and swapping the two streams
    fn renormalize(&mut self) {
        let mut x = self.bits_a;
        if x < 0x80000000 {
            x = (x << 32) | self.read() as u64;
        }
        self.bits_a = self.bits_b;
        self.bits_b = x;
    }

    /// Read a single bit with a uniform distribution.
    fn read_bool(&mut self) -> bool {
        let r = self.bits_a & 1;
        self.bits_a >>= 1;
        self.renormalize();
        r == 1
    }

    /// Read a number of bits with a uniform distribution.
    fn read_n_bits(&mut self, bits: usize) -> usize {
        let rv = self.bits_a & ((1 << bits) - 1);
        self.bits_a >>= bits;
        self.renormalize();
        rv as usize
    }

    /// Read a 4-bit value using an adaptive RANS model
    fn read_nibble(&mut self, model: &mut LznaNibbleModel) -> usize {
        let x = self.bits_a;
        let bitindex;
        let start;
        let end;

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            #[cfg(target_arch = "x86")]
            use std::arch::x86::*;
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::*;

            let t0 = _mm_loadu_si128(std::ptr::addr_of!(model.prob[0]).cast());
            let t1 = _mm_loadu_si128(std::ptr::addr_of!(model.prob[8]).cast());

            let t = _mm_cvtsi32_si128(x as i32 & 0x7FFF);
            let t = _mm_shuffle_epi32::<0>(_mm_unpacklo_epi16(t, t));

            let c0 = _mm_cmpgt_epi16(t0, t);
            let c1 = _mm_cmpgt_epi16(t1, t);

            let m = _mm_movemask_epi8(_mm_packs_epi16(c0, c1));

            bitindex = (m | 0x10000).trailing_zeros() as usize;
            start = model.prob[bitindex - 1] as u64;
            end = model.prob[bitindex] as u64;

            let c0 = _mm_and_si128(_mm_set1_epi16(0x7FD9), c0);
            let c1 = _mm_and_si128(_mm_set1_epi16(0x7FD9), c1);

            let c0 = _mm_add_epi16(c0, _mm_set_epi16(56, 48, 40, 32, 24, 16, 8, 0));
            let c1 = _mm_add_epi16(c1, _mm_set_epi16(120, 112, 104, 96, 88, 80, 72, 64));

            let t0 = _mm_add_epi16(_mm_srai_epi16::<7>(_mm_sub_epi16(c0, t0)), t0);
            let t1 = _mm_add_epi16(_mm_srai_epi16::<7>(_mm_sub_epi16(c1, t1)), t1);

            _mm_storeu_si128(std::ptr::addr_of_mut!(model.prob[0]).cast(), t0);
            _mm_storeu_si128(std::ptr::addr_of_mut!(model.prob[8]).cast(), t1);
        }

        self.bits_a = (end - start) * (x >> 15) + (x & 0x7FFF) - start;
        self.renormalize();
        bitindex - 1
    }

    /// Read a 3-bit value using an adaptive RANS model
    fn read_3_bits(&mut self, model: &mut Lzna3bitModel) -> usize {
        let bitindex;
        let start;
        let end;
        let x = self.bits_a;

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            #[cfg(target_arch = "x86")]
            use std::arch::x86::*;
            #[cfg(target_arch = "x86_64")]
            use std::arch::x86_64::*;
            let t0 = _mm_loadu_si128(std::ptr::addr_of!(model.prob[0]).cast());
            let t = _mm_cvtsi32_si128(x as i32 & 0x7FFF);
            let t = _mm_shuffle_epi32::<0>(_mm_unpacklo_epi16(t, t));
            let c0 = _mm_cmpgt_epi16(t0, t);

            bitindex = (_mm_movemask_epi8(c0) | 0x10000).trailing_zeros() as usize >> 1;
            start = model.prob[bitindex - 1] as u64;
            end = model.prob[bitindex] as u64;

            let c0 = _mm_and_si128(_mm_set1_epi16(0x7FE5), c0);
            let c0 = _mm_add_epi16(c0, _mm_set_epi16(56, 48, 40, 32, 24, 16, 8, 0));
            let t0 = _mm_add_epi16(_mm_srai_epi16::<7>(_mm_sub_epi16(c0, t0)), t0);
            _mm_storeu_si128(std::ptr::addr_of!(model.prob[0]).cast_mut().cast(), t0);
        }

        self.bits_a = (end - start) * (x >> 15) + (x & 0x7FFF) - start;
        self.renormalize();
        bitindex - 1
    }

    /// Read a 1-bit value using an adaptive RANS model
    fn read_1_bit(&mut self, model: &mut LznaBitModel, nbits: i32, shift: i32) -> usize {
        assert!(nbits < 32);
        let magn = 1u64 << nbits;
        let q = *model as u64 * (self.bits_a >> nbits);
        if (self.bits_a & (magn - 1)) >= *model as u64 {
            self.bits_a -= q + *model as u64;
            *model = *model - (*model >> shift);
            self.renormalize();
            1
        } else {
            self.bits_a = (self.bits_a & (magn - 1)) + q;
            *model += ((magn - *model as u64) >> shift) as LznaBitModel;
            self.renormalize();
            0
        }
    }

    /// Read a far distance using the far distance model
    fn read_far_distance(&mut self, lut: &mut LznaState) -> usize {
        let mut n = self.read_nibble(&mut lut.far_distance.first_lo);
        let mut hi;
        if n >= 15 {
            n = 15 + self.read_nibble(&mut lut.far_distance.first_hi);
        }
        hi = 0;
        if n != 0 {
            hi = self.read_1_bit(&mut lut.far_distance.second[n - 1], 14, 6) + 2;
            if n != 1 {
                hi = (hi << 1) + self.read_1_bit(&mut lut.far_distance.third[hi - 2][n - 1], 14, 6);
                if n != 2 {
                    hi = (hi << (n - 2)) + self.read_n_bits(n - 2);
                }
            }
            hi -= 1;
        }
        let lutd = &mut lut.low_bits_of_distance[if hi == 0 { 1 } else { 0 }];
        let low_bit = self.read_1_bit(&mut lutd.v, 14, 6);
        let low_nibble = self.read_nibble(&mut lutd.d[low_bit]);
        low_bit + (2 * low_nibble) + (32 * hi) + 1
    }

    /// Read a near distance using a near distance model
    fn read_near_distance(&mut self, lut: &mut LznaState, idx: usize) -> usize {
        let model = &mut lut.near_dist[idx];
        let nb = self.read_nibble(&mut model.first);
        let mut hi = 0;
        if nb != 0 {
            hi = self.read_1_bit(&mut model.second[nb - 1], 14, 6) + 2;
            if nb != 1 {
                hi = (hi << 1) + self.read_1_bit(&mut model.third[hi - 2][nb - 1], 14, 6);
                if nb != 2 {
                    hi = (hi << (nb - 2)) + self.read_n_bits(nb - 2);
                }
            }
            hi -= 1;
        }
        let lutd = &mut lut.low_bits_of_distance[if hi == 0 { 1 } else { 0 }];
        let low_bit = self.read_1_bit(&mut lutd.v, 14, 6);
        let low_nibble = self.read_nibble(&mut lutd.d[low_bit]);
        low_bit + (2 * low_nibble) + (32 * hi) + 1
    }

    /// Read a length using the length model.
    fn read_length(&mut self, model: &mut LznaLongLengthModel) -> usize {
        let mut length = self.read_nibble(&mut model.first[self.dst & 3]);
        if length >= 12 {
            let mut b = self.read_nibble(&mut model.second);
            if b >= 15 {
                b = 15 + self.read_nibble(&mut model.third);
            }
            let mut n = 0;
            let mut base = 0;
            if b != 0 {
                n = (b - 1) >> 1;
                base = ((((b - 1) & 1) + 2) << n) - 1;
            }
            length += (self.read_n_bits(n) + base) * 4;
        }
        length
    }

    pub(crate) fn decode_quantum(&mut self, lut: &mut LznaState) -> Result<usize, OozError> {
        lut.preprocess_match_history();
        self.init();
        let mut dist = lut.match_history[4] as usize;

        let mut state = 5;
        let dst_end = self.output.len() - 8;
        let mut x;

        if self.dst == 0 {
            if self.read_bool() {
                x = 0;
            } else {
                let model = &mut lut.literal[0];
                x = self.read_nibble(&mut model.upper[0]);
                x = (x << 4)
                    + self.read_nibble(if x != 0 {
                        &mut model.nomatch[x]
                    } else {
                        &mut model.lower[0]
                    });
            }
            self.write(x as u8);
        }
        while self.dst < dst_end {
            let match_val = self.output[self.dst - dist];

            if self.read_1_bit(&mut lut.is_literal[(self.dst & 7) + 8 * state], 13, 5) != 0 {
                x = self.read_nibble(&mut lut.typ[(self.dst & 7) + 8 * state]);
                if x == 0 {
                    // Copy 1 byte from most recent distance
                    self.write(match_val);
                    state = if state >= 7 { 11 } else { 9 };
                } else if x < 4 {
                    if x == 1 {
                        // Copy count 3-4
                        let length =
                            3 + self.read_1_bit(&mut lut.short_length[state][self.dst & 3], 14, 4);
                        dist = self.read_near_distance(lut, length - 3);
                        self.copy_offset(dist, length);
                    } else if x == 2 {
                        // Copy count 5-12
                        let length = 5 + self.read_3_bits(&mut lut.medium_length);
                        dist = self.read_far_distance(lut);
                        self.copy_offset(dist, length);
                    } else {
                        // Copy count 13-
                        let length = self.read_length(&mut lut.long_length) + 13;
                        dist = self.read_far_distance(lut);
                        self.copy_offset(dist, length);
                    }
                    state = if state >= 7 { 10 } else { 7 };
                    lut.match_history[7] = lut.match_history[6];
                    lut.match_history[6] = lut.match_history[5];
                    lut.match_history[5] = lut.match_history[4];
                    lut.match_history[4] = dist as u32;
                } else if x >= 12 {
                    // Copy 2 bytes from a recent distance
                    let idx = x - 12;
                    dist = lut.match_history[4 + idx] as usize;
                    lut.match_history[4 + idx] = lut.match_history[3 + idx];
                    lut.match_history[3 + idx] = lut.match_history[2 + idx];
                    lut.match_history[2 + idx] = lut.match_history[1 + idx];
                    lut.match_history[4] = dist as u32;
                    self.copy_offset(dist, 2);
                    state = if state >= 7 { 11 } else { 8 };
                } else {
                    let idx = (x - 4) >> 1;
                    dist = lut.match_history[4 + idx] as usize;
                    lut.match_history[4 + idx] = lut.match_history[3 + idx];
                    lut.match_history[3 + idx] = lut.match_history[2 + idx];
                    lut.match_history[2 + idx] = lut.match_history[1 + idx];
                    lut.match_history[4] = dist as u32;
                    if x & 1 == 1 {
                        // Copy 11- bytes from recent distance
                        let length = 11 + self.read_length(&mut lut.long_length_recent);
                        self.copy_offset(dist, length);
                    } else {
                        // Copy 3-10 bytes from recent distance
                        let length =
                            3 + self.read_3_bits(&mut lut.short_length_recent[idx].a[self.dst & 3]);
                        self.copy_offset(dist, length);
                    }
                    state = if state >= 7 { 11 } else { 8 };
                }
            } else {
                // Output a literal
                let model = &mut lut.literal[self.dst & 3];
                x = self.read_nibble(&mut model.upper[match_val as usize >> 4]);
                x = (x << 4)
                    + self.read_nibble(if (match_val as usize >> 4) != x {
                        &mut model.nomatch[x]
                    } else {
                        &mut model.lower[match_val as usize & 0xF]
                    });
                self.write(x as u8);
                state = [0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 4, 5][state];
            }
        }

        self.assert_eq(self.dst, dst_end)?;

        self.output[self.dst..][..4].copy_from_slice(&(self.bits_a as i32).to_le_bytes());
        self.output[self.dst + 4..].copy_from_slice(&(self.bits_b as i32).to_le_bytes());

        Ok(self.src)
    }
}
