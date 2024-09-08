#![allow(non_snake_case)]

use crate::{KrakenDecoder, Pointer};

pub struct BitReader {
    /// |p| holds the current u8 and |p_end| the end of the buffer.
    pub p: Pointer,
    pub p_end: Pointer,
    /// Bits accumulated so far
    pub bits: u32,
    /// Next u8 will end up in the |bitpos| position in |bits|.
    pub bitpos: i32,
}

pub struct BitReader2 {
    pub p: Pointer,
    pub p_end: Pointer,
    pub bitpos: u32,
}

impl BitReader {
    // Read more bytes to make sure we always have at least 24 bits in |bits|.
    pub fn Refill(&mut self, source: &KrakenDecoder) {
        assert!(self.bitpos <= 24);
        while self.bitpos > 0 {
            self.bits |= (if self.p < self.p_end {
                source.get_byte(self.p).into()
            } else {
                0
            }) << self.bitpos;
            self.bitpos -= 8;
            self.p += 1;
        }
    }

    // Read more bytes to make sure we always have at least 24 bits in |bits|,
    // used when reading backwards.
    pub fn RefillBackwards(&mut self, source: &KrakenDecoder) {
        assert!(self.bitpos <= 24);
        while self.bitpos > 0 {
            self.p -= 1;
            self.bits |= (if self.p >= self.p_end {
                source.get_byte(self.p).into()
            } else {
                0
            }) << self.bitpos;
            self.bitpos -= 8;
        }
    }

    // Refill bits then read a single bit.
    pub fn ReadBit(&mut self, source: &KrakenDecoder) -> bool {
        self.Refill(source);
        let r = self.bits >> 31;
        self.bits <<= 1;
        self.bitpos += 1;
        r != 0
    }

    pub fn ReadBitNoRefill(&mut self) -> bool {
        let r = self.bits >> 31;
        self.bits <<= 1;
        self.bitpos += 1;
        r != 0
    }

    // Read |n| bits without refilling.
    pub fn ReadBitsNoRefill(&mut self, n: i32) -> i32 {
        let r = self.bits >> (32 - n);
        self.bits <<= n;
        self.bitpos += n;
        r as _
    }

    // Read |n| bits without refilling, n may be zero.
    pub fn ReadBitsNoRefillZero(&mut self, n: i32) -> i32 {
        let r = self.bits >> 1 >> (31 - n);
        self.bits <<= n;
        self.bitpos += n;
        r as _
    }

    pub fn ReadMoreThan24Bits(&mut self, source: &KrakenDecoder, n: i32) -> i32 {
        let mut rv;
        if n <= 24 {
            rv = self.ReadBitsNoRefillZero(n);
        } else {
            rv = self.ReadBitsNoRefill(24) << (n - 24);
            self.Refill(source);
            rv += self.ReadBitsNoRefill(n - 24);
        }
        self.Refill(source);
        rv
    }

    pub fn ReadMoreThan24BitsB(&mut self, source: &KrakenDecoder, n: i32) -> i32 {
        let mut rv;
        if n <= 24 {
            rv = self.ReadBitsNoRefillZero(n);
        } else {
            rv = self.ReadBitsNoRefill(24) << (n - 24);
            self.RefillBackwards(source);
            rv += self.ReadBitsNoRefill(n - 24);
        }
        self.RefillBackwards(source);
        rv
    }

    // Reads a gamma value.
    // Assumes bitreader is already filled with at least 23 bits
    pub fn ReadGamma(&mut self) -> i32 {
        let mut n;
        if self.bits != 0 {
            n = self.high_bit();
        } else {
            n = 32;
        }
        n = 2 * n + 2;
        assert!(n < 24);
        self.bitpos += n;
        let r = (self.bits >> (32 - n)) as i32;
        self.bits <<= n;
        r - 2
    }

    // Reads a gamma value with |forced| number of forced bits.
    pub fn ReadGammaX(&mut self, forced: i32) -> i32 {
        if self.bits != 0 {
            let lz = self.high_bit();
            assert!(lz < 24);
            let r = (self.bits >> (31 - lz - forced)) + ((lz as u32 - 1) << forced);
            self.bits <<= lz + forced + 1;
            self.bitpos += lz + forced + 1;
            return r as _;
        }
        0
    }

    // Reads an offset code parametrized by |v|.
    pub fn ReadDistance(&mut self, source: &KrakenDecoder, v: i32) -> i32 {
        let w;
        let m;
        let n;
        let mut rv;
        if v < 0xF0 {
            n = (v >> 4) + 4;
            w = (self.bits | 1).rotate_left(n as u32);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = ((w & m) << 4) + (v & 0xF) as u32 - 248;
        } else {
            n = v - 0xF0 + 4;
            w = (self.bits | 1).rotate_left(n as u32);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = 8322816 + ((w & m) << 12);
            self.Refill(source);
            rv += self.bits >> 20;
            self.bitpos += 12;
            self.bits <<= 12;
        }
        self.Refill(source);
        rv as _
    }

    // Reads an offset code parametrized by |v|, backwards.
    pub fn ReadDistanceB(&mut self, source: &KrakenDecoder, v: i32) -> i32 {
        let w;
        let m;
        let n;
        let mut rv;

        if v < 0xF0 {
            n = (v >> 4) + 4;
            w = (self.bits | 1).rotate_left(n as u32);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = ((w & m) << 4) + (v & 0xF) as u32 - 248;
        } else {
            n = v - 0xF0 + 4;
            w = (self.bits | 1).rotate_left(n as u32);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = 8322816 + ((w & m) << 12);
            self.RefillBackwards(source);
            rv += self.bits >> (32 - 12);
            self.bitpos += 12;
            self.bits <<= 12;
        }
        self.RefillBackwards(source);
        rv as _
    }

    // Reads a length code.
    pub fn ReadLength(&mut self, source: &KrakenDecoder) -> Option<i32> {
        let mut n;
        n = self.high_bit();
        if n > 12 {
            return None;
        }
        self.bitpos += n;
        self.bits <<= n;
        self.Refill(source);
        n += 7;
        self.bitpos += n;
        let rv = (self.bits >> (32 - n)) - 64;
        self.bits <<= n;
        self.Refill(source);
        Some(rv as _)
    }

    // Reads a length code, backwards.
    pub fn ReadLengthB(&mut self, source: &KrakenDecoder) -> Option<i32> {
        let mut n = self.high_bit();
        if n > 12 {
            return None;
        }
        self.bitpos += n;
        self.bits <<= n;
        self.RefillBackwards(source);
        n += 7;
        self.bitpos += n;
        let rv = (self.bits >> (32 - n)) - 64;
        self.bits <<= n;
        self.RefillBackwards(source);
        Some(rv as _)
    }

    pub fn ReadFluff(&mut self, num_symbols: i32) -> usize {
        if num_symbols == 256 {
            return 0;
        }

        let mut x = 257 - num_symbols;
        if x > num_symbols {
            x = num_symbols;
        }

        x *= 2;

        let h = (x - 1i32).ilog2();
        let y = 32 - h;

        let v = self.bits >> h;
        let z = (1 << y) - x as u32;

        if (v >> 1) >= z {
            self.bits <<= y;
            self.bitpos += y as i32;
            (v - z) as _
        } else {
            self.bits <<= y - 1;
            self.bitpos += (y - 1) as i32;
            (v >> 1) as _
        }
    }

    pub fn high_bit(&self) -> i32 {
        self.bits.ilog2() as _
    }
}
