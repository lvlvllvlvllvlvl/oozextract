use crate::error::OozError;
use std::mem::size_of;

#[derive(Copy, Clone)]
struct Base<const F: usize, const A: usize, const L: usize> {
    a: [u16; A],
    freq: [u16; F],
    adapt_interval: u16,
    lookup: [u16; L],
}

type Literal = Base<300, 301, 516>;

type DistanceLsb = Base<40, 41, 68>;

type DistanceBits = Base<21, 22, 68>;

impl<const F: usize, const A: usize, const L: usize> Base<F, A, L> {
    const SHIFT: u16 = if A == 301 { 6 } else { 9 };
    const F_INC: u16 = 1026 - A as u16;

    fn fill_lut(&mut self) {
        let mut p = 0;
        for (v, i) in self.a[1..].iter().zip(0u16..) {
            let p_end = ((v - 1) >> Self::SHIFT) + 1;
            for j in (p as usize..=p_end as usize).step_by(4) {
                self.lookup[j..][..4].fill(i);
            }
            p = p_end;
        }
    }

    fn adapt(&mut self, sym: usize) {
        self.adapt_interval = 1024;
        self.freq[sym] += Self::F_INC;

        let mut sum = 0;
        for (f, a) in self.freq.iter_mut().zip(self.a[1..].iter_mut()) {
            sum += *f as u32;
            *a = (*a as u32).wrapping_add(sum.wrapping_sub(*a as u32) >> 1) as u16;
        }
        self.freq.fill(1);

        self.fill_lut();
    }

    fn lookup(&mut self, bits: &mut u32) -> usize {
        let masked = (*bits & 0x7FFF) as usize;
        let mut sym = self.lookup[masked >> Self::SHIFT] as usize;
        if masked > self.a[sym + 1] as usize {
            sym += 1;
        }
        while masked >= self.a[sym + 1] as usize {
            sym += 1;
        }
        let s = self.a[sym] as u32;
        let s1 = self.a[sym + 1] as u32;
        *bits = masked as u32 + (*bits >> 15) * (s1 - s) - s;
        self.freq[sym] += 31;
        self.adapt_interval -= 1;
        if self.adapt_interval == 0 {
            self.adapt(sym);
        }
        sym
    }
}

impl<const F: usize, const A: usize, const L: usize> Default for Base<F, A, L> {
    fn default() -> Self {
        let a = if Self::SHIFT == 6 {
            core::array::from_fn(|i| {
                if i < 264 {
                    ((0x8000 - 300 + 264) * i / 264) as u16
                } else {
                    ((0x8000 - 300) + i) as u16
                }
            })
        } else {
            core::array::from_fn(|i| (0x8000 * i / F) as u16)
        };

        let mut s = Self {
            a,
            freq: [1; F],
            adapt_interval: 1024,
            lookup: [0; L],
        };

        s.fill_lut();
        s
    }
}

pub(crate) struct State {
    recent_dist: [u32; 8],
    last_match_dist: u32,
    recent_dist_mask: u32,

    literals: [Literal; 4],
    distance_lsb: [DistanceLsb; 4],
    distance_bits: DistanceBits,
}

impl State {
    pub(crate) fn new() -> Self {
        Self {
            last_match_dist: 1,
            recent_dist: [1; 8],
            recent_dist_mask: (1 << 3)
                | (2 << (2 * 3))
                | (3 << (3 * 3))
                | (4 << (4 * 3))
                | (5 << (5 * 3))
                | (6 << (6 * 3))
                | (7 << (7 * 3)),
            literals: Default::default(),
            distance_lsb: Default::default(),
            distance_bits: Default::default(),
        }
    }
}

pub(crate) struct Core<'a> {
    state: &'a mut State,
    input: &'a [u8],
    output: &'a mut [u8],
    src: usize,
    dst: usize,
    bits: u32,
    bits2: u32,
    litmodel: [usize; 4],
    distancelsb: [usize; 4],
}

impl<'a> Core<'a> {
    pub(crate) fn new(
        input: &'a [u8],
        output: &'a mut [u8],
        state: &'a mut State,
        dst: usize,
    ) -> Core<'a> {
        Self {
            state,
            input,
            output,
            src: 0,
            dst,
            bits: 0x10000,
            bits2: 0x10000,
            litmodel: core::array::from_fn(|i| i),
            distancelsb: core::array::from_fn(|i| i),
        }
    }

    fn read_2(&mut self) -> u32 {
        let v = u16::from_le_bytes(*self.input[self.src..].first_chunk().unwrap());
        self.src += size_of::<u16>();
        v as u32
    }

    fn read_4(&mut self) -> u32 {
        let v = u32::from_le_bytes(*self.input[self.src..].first_chunk().unwrap());
        self.src += size_of::<u32>();
        v
    }

    fn write_1(&mut self, v: u8) {
        self.output[self.dst] = v;
        self.dst += 1;
    }

    fn write_2(&mut self, v: u16) {
        self.output[self.dst..][..2].copy_from_slice(&v.to_le_bytes());
        self.dst += 2;
    }

    fn write_sym(&mut self, sym: u8) {
        self.output[self.dst] = sym.wrapping_add(self.last_match());
        self.dst += 1;
    }

    fn copy_chunks<const CHUNK_SIZE: usize>(&mut self, copy_length: usize, match_dist: usize) {
        for i in 0..copy_length / CHUNK_SIZE {
            let dst = self.dst + i * CHUNK_SIZE;
            let src = dst - match_dist;
            self.output.copy_within(src..src + CHUNK_SIZE, dst);
        }
        let rem = copy_length % CHUNK_SIZE;
        let dst = self.dst + copy_length - rem;
        let src = dst - match_dist;
        self.output.copy_within(src..src + rem, dst);
    }

    fn last_match(&self) -> u8 {
        self.output[self.dst - self.state.last_match_dist as usize]
    }

    fn lookup_literal(&mut self) -> usize {
        self.state.literals[self.litmodel[self.dst & 3]].lookup(&mut self.bits)
    }

    fn lookup_lsb(&mut self) -> usize {
        self.state.distance_lsb[self.distancelsb[self.dst & 3]].lookup(&mut self.bits)
    }

    fn lookup_bits(&mut self) -> usize {
        self.state.distance_bits.lookup(&mut self.bits)
    }

    fn renormalize(&mut self) {
        if self.bits < 0x10000 {
            self.bits = (self.bits << 16) | self.read_2();
        }
        std::mem::swap(&mut self.bits, &mut self.bits2);
    }

    pub(crate) fn decode(&mut self) -> Result<usize, OozError> {
        let mut recent_mask = self.state.recent_dist_mask as usize;

        let v = self.read_4();
        if v < 0x10000 {
            return Ok(0);
        }

        let mut a = v >> 4;
        let n = v & 0xF;
        if a < 0x10000 {
            a = (a << 16) | self.read_2();
        }
        self.bits = a >> n;
        if self.bits < 0x10000 {
            self.bits = (self.bits << 16) | self.read_2();
        }
        a = (a << 16) | self.read_2();

        self.bits2 = (1 << (n + 16)) | (a & ((1 << (n + 16)) - 1));

        if self.dst == 0 {
            self.write_1(self.bits as u8);
            self.bits >>= 8;
            self.renormalize();
        }

        while self.dst + 4 < self.output.len() {
            let mut sym = self.lookup_literal();
            self.renormalize();

            if sym < 256 {
                self.write_sym(sym as u8);

                if self.dst + 4 >= self.output.len() {
                    break;
                }

                sym = self.lookup_literal();
                self.renormalize();

                if sym < 256 {
                    self.write_sym(sym as u8);
                    continue;
                }
            }

            if sym >= 288 {
                let nb = sym - 287;
                sym = (self.bits as usize & ((1 << nb) - 1)) + (1 << nb) + 286;
                self.bits >>= nb;
                self.renormalize();
            }

            let copy_length = sym - 254;

            sym = self.lookup_lsb();
            self.renormalize();

            let mut match_dist;
            if sym >= 8 {
                let nb = self.lookup_bits();
                self.renormalize();

                match_dist = self.bits & ((1 << (nb & 0xF)) - 1);
                self.bits >>= nb & 0xF;
                self.renormalize();
                if nb >= 0x10 {
                    match_dist = (match_dist << 16) | self.read_2();
                }
                match_dist = (32 << nb) + (match_dist << 5) + sym as u32 - 39;

                self.state.recent_dist[(recent_mask >> 21) & 7] =
                    self.state.recent_dist[(recent_mask >> 18) & 7];
                self.state.recent_dist[(recent_mask >> 18) & 7] = match_dist;
            } else {
                let idx = (recent_mask >> (3 * sym)) & 7;
                let mask = !7 << (3 * sym);
                match_dist = self.state.recent_dist[idx];
                recent_mask = (recent_mask & mask) | ((idx + 8 * recent_mask) & !mask);
            }

            if match_dist == 1 {
                let v = self.output[self.dst - 1];
                self.output[self.dst..][..copy_length].fill(v);
            } else if match_dist as usize > copy_length {
                let src = self.dst - match_dist as usize;
                self.output.copy_within(src..src + copy_length, self.dst);
            } else if match_dist >= 8 {
                self.copy_chunks::<8>(copy_length, match_dist as usize);
            } else if match_dist >= 4 {
                self.copy_chunks::<4>(copy_length, match_dist as usize);
            } else {
                for i in 0..copy_length {
                    self.output[self.dst + i] = self.output[self.dst + i - match_dist as usize];
                }
            }

            self.dst += copy_length;
            self.state.last_match_dist = match_dist;
        }
        self.write_2(self.bits as u16);
        self.write_2(self.bits2 as u16);

        self.state.recent_dist_mask = recent_mask as u32;
        Ok(self.src)
    }
}
