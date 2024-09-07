use std::{default, process::Output, usize};

// Kraken decompression happens in two phases, first one decodes
// all the literals and copy lengths using huffman and second
// phase runs the copy loop. This holds the tables needed by stage 2.
#[derive(Default)]
pub struct KrakenLzTable {
    // Stream of (literal, match) pairs. The flag u8 contains
    // the length of the match, the length of the literal and whether
    // to use a recent offset.
    cmd_stream: Pointer,
    cmd_stream_size: usize,

    // Holds the actual distances in case we're not using a recent
    // offset.
    offs_stream: IntPointer,
    offs_stream_size: usize,

    // Holds the sequence of literals. All literal copying happens from
    // here.
    lit_stream: Pointer,
    lit_stream_size: usize,

    // Holds the lengths that do not fit in the flag stream. Both literal
    // lengths and match length are stored in the same array.
    len_stream: IntPointer,
    len_stream_size: usize,
}

pub struct KrakenDecoder {
    input: Vec<u8>,
    output: Vec<u8>,
    scratch: Vec<u8>,
}

struct BitReader {
    /// |p| holds the current u8 and |p_end| the end of the buffer.
    pub p: Pointer,
    pub p_end: Pointer,
    /// Bits accumulated so far
    pub bits: u32,
    /// Next u8 will end up in the |bitpos| position in |bits|.
    pub bitpos: u32,
}
impl BitReader {
    // Read more bytes to make sure we always have at least 24 bits in |bits|.
    fn Refill(&mut self, source: &mut KrakenDecoder) {
        assert!(self.bitpos <= 24);
        while self.bitpos > 0 {
            self.bits |= (if self.p < self.p_end {
                source.get_as_u32(self.p)
            } else {
                0
            }) << self.bitpos;
            self.bitpos -= 8;
            self.p += 1;
        }
    }

    // Read more bytes to make sure we always have at least 24 bits in |bits|,
    // used when reading backwards.
    fn RefillBackwards(&mut self, source: &mut KrakenDecoder) {
        assert!(self.bitpos <= 24);
        while self.bitpos > 0 {
            self.p -= 1;
            self.bits |= (if self.p >= self.p_end {
                source.get_as_u32(self.p)
            } else {
                0
            }) << self.bitpos;
            self.bitpos -= 8;
        }
    }

    // Refill bits then read a single bit.
    fn ReadBit(&mut self, source: &mut KrakenDecoder) -> u32 {
        let r;
        self.Refill(source);
        r = self.bits >> 31;
        self.bits <<= 1;
        self.bitpos += 1;
        return r;
    }

    fn ReadBitNoRefill(&mut self) -> u32 {
        let r;
        r = self.bits >> 31;
        self.bits <<= 1;
        self.bitpos += 1;
        return r;
    }

    // Read |n| bits without refilling.
    fn ReadBitsNoRefill(&mut self, n: u32) -> u32 {
        let r = self.bits >> (32 - n);
        self.bits <<= n;
        self.bitpos += n;
        return r;
    }

    // Read |n| bits without refilling, n may be zero.
    fn ReadBitsNoRefillZero(&mut self, n: u32) -> u32 {
        let r = self.bits >> 1 >> (31 - n);
        self.bits <<= n;
        self.bitpos += n;
        return r;
    }

    fn ReadMoreThan24Bits(&mut self, source: &mut KrakenDecoder, n: u32) -> u32 {
        let mut rv;
        if n <= 24 {
            rv = self.ReadBitsNoRefillZero(n);
        } else {
            rv = self.ReadBitsNoRefill(24) << (n - 24);
            self.Refill(source);
            rv += self.ReadBitsNoRefill(n - 24);
        }
        self.Refill(source);
        return rv;
    }

    fn ReadMoreThan24BitsB(&mut self, source: &mut KrakenDecoder, n: u32) -> u32 {
        let mut rv;
        if n <= 24 {
            rv = self.ReadBitsNoRefillZero(n);
        } else {
            rv = self.ReadBitsNoRefill(24) << (n - 24);
            self.RefillBackwards(source);
            rv += self.ReadBitsNoRefill(n - 24);
        }
        self.RefillBackwards(source);
        return rv;
    }

    // Reads a gamma value.
    // Assumes bitreader is already filled with at least 23 bits
    fn ReadGamma(&mut self) -> u32 {
        let mut n;
        let r;
        if self.bits != 0 {
            n = self.bits.ilog2();
        } else {
            n = 32;
        }
        n = 2 * n + 2;
        assert!(n < 24);
        self.bitpos = self
            .bitpos
            .checked_add_signed(n.try_into().unwrap())
            .unwrap();
        r = self.bits >> (32 - n);
        self.bits <<= n;
        return r - 2;
    }

    // Reads a gamma value with |forced| number of forced bits.
    fn ReadGammaX(&mut self, forced: u32) -> u32 {
        let r;
        if self.bits != 0 {
            let lz = self.bits.ilog2();
            assert!(lz < 24);
            r = (self.bits >> (31 - lz - forced)) + ((lz - 1) << forced);
            self.bits <<= lz + forced + 1;
            self.bitpos += lz + forced + 1;
            return r;
        }
        return 0;
    }

    // Reads a offset code parametrized by |v|.
    fn ReadDistance(&mut self, source: &mut KrakenDecoder, v: u32) -> u32 {
        let w;
        let m;
        let n;
        let mut rv;
        if v < 0xF0 {
            n = (v >> 4) + 4;
            w = (self.bits | 1).rotate_left(n);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = ((w & m) << 4) + (v & 0xF) - 248;
        } else {
            n = v - 0xF0 + 4;
            w = (self.bits | 1).rotate_left(n);
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
        return rv;
    }

    // Reads a offset code parametrized by |v|, backwards.
    fn ReadDistanceB(&mut self, source: &mut KrakenDecoder, v: u32) -> u32 {
        let w;
        let m;
        let n;
        let mut rv;
        if v < 0xF0 {
            n = (v >> 4) + 4;
            w = (self.bits | 1).rotate_left(n);
            self.bitpos += n;
            m = (2 << n) - 1;
            self.bits = w & !m;
            rv = ((w & m) << 4) + (v & 0xF) - 248;
        } else {
            n = v - 0xF0 + 4;
            w = (self.bits | 1).rotate_left(n);
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
        return rv;
    }

    // Reads a length code.
    fn ReadLength(&mut self, source: &mut KrakenDecoder) -> Option<u32> {
        let mut n;
        n = self.bits.ilog2();
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
        return Some(rv);
    }

    // Reads a length code, backwards.
    fn ReadLengthB(&mut self, source: &mut KrakenDecoder) -> Option<u32> {
        let mut n = self.bits.ilog2();
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
        return Some(rv);
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
enum PointerDest {
    #[default]
    Null,
    Input,
    Output,
    Scratch,
}

impl PartialOrd for PointerDest {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self == other {
            Some(std::cmp::Ordering::Equal)
        } else {
            None
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd)]
struct Pointer {
    into: PointerDest,
    index: usize,
}

impl Pointer {
    fn input(index: usize) -> Self {
        Pointer {
            into: PointerDest::Input,
            index,
        }
    }
    fn output(index: usize) -> Self {
        Pointer {
            into: PointerDest::Output,
            index,
        }
    }
    fn scratch(index: usize) -> Self {
        Pointer {
            into: PointerDest::Scratch,
            index,
        }
    }
}

impl std::ops::Add<usize> for Pointer {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index + rhs,
            ..self
        }
    }
}

impl std::ops::Add<usize> for &Pointer {
    type Output = Pointer;

    fn add(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index + rhs,
            ..*self
        }
    }
}

impl std::ops::Add<i32> for Pointer {
    type Output = Self;

    fn add(self, rhs: i32) -> Self::Output {
        Pointer {
            index: self
                .index
                .checked_add_signed(rhs.try_into().unwrap())
                .unwrap(),
            ..self
        }
    }
}

impl std::ops::Add<isize> for Pointer {
    type Output = Self;

    fn add(self, rhs: isize) -> Self::Output {
        Pointer {
            index: self.index.checked_add_signed(rhs).unwrap(),
            ..self
        }
    }
}

impl std::ops::AddAssign<usize> for Pointer {
    fn add_assign(&mut self, rhs: usize) {
        self.index += rhs
    }
}

impl std::ops::SubAssign<usize> for Pointer {
    fn sub_assign(&mut self, rhs: usize) {
        self.index -= rhs
    }
}

impl std::ops::Sub<Pointer> for Pointer {
    type Output = usize;

    fn sub(self, rhs: Pointer) -> Self::Output {
        assert!(self.into == rhs.into);
        self.index - rhs.index
    }
}

impl std::ops::Sub<usize> for Pointer {
    type Output = Pointer;

    fn sub(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index - rhs,
            ..self
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd)]
struct IntPointer {
    into: PointerDest,
    index: usize,
}

impl IntPointer {
    fn add_byte_offset(self, rhs: usize) -> Self {
        IntPointer {
            index: self.index + rhs,
            ..self
        }
    }
}

impl std::ops::Add<usize> for IntPointer {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        IntPointer {
            index: self.index + (rhs * 4),
            ..self
        }
    }
}

impl std::ops::Add<usize> for &IntPointer {
    type Output = IntPointer;

    fn add(self, rhs: usize) -> Self::Output {
        IntPointer {
            index: self.index + (rhs * 4),
            ..*self
        }
    }
}

impl std::ops::AddAssign<usize> for IntPointer {
    fn add_assign(&mut self, rhs: usize) {
        self.index += rhs * 4
    }
}

impl std::ops::Sub<IntPointer> for IntPointer {
    type Output = usize;

    fn sub(self, rhs: IntPointer) -> Self::Output {
        assert!(self.into == rhs.into);
        (self.index - rhs.index) / 4
    }
}

impl std::ops::Sub<usize> for IntPointer {
    type Output = IntPointer;

    fn sub(self, rhs: usize) -> Self::Output {
        IntPointer {
            index: self.index - (rhs * 4),
            ..self
        }
    }
}

impl From<Pointer> for IntPointer {
    fn from(value: Pointer) -> Self {
        IntPointer {
            into: value.into,
            index: value.index,
        }
    }
}

impl KrakenDecoder {
    // Decode one 256kb big quantum block. It's divided into two 128k blocks
    // internally that are compressed separately but with a shared history.
    pub fn decode_quantum(&mut self, write_from: usize, write_to: usize) -> usize {
        let mut written_bytes = 0;
        let mut src = Pointer::output(0);
        let src_in = Pointer::input(0);
        let src_end = Pointer::output(self.input.len());
        let mut dst = Pointer::output(0);
        let dst_start = Pointer::output(write_from);
        let dst_end = Pointer::output(write_to);
        let scratch = Pointer::output(0);
        let scratch_end = Pointer::output(self.scratch.len());
        let mut src_used = 0;

        while dst_end - dst != 0 {
            let dst_count = std::cmp::min(dst_end - dst, 0x20000);
            if src_end - src < 4 {
                panic!()
            }
            let chunkhdr = self.get_as_usize(src + 2)
                | self.get_as_usize(src + 1) << 8
                | self.get_as_usize(src + 0) << 16;
            if (chunkhdr & 0x800000) != 0 {
                // Stored as entropy without any match copying.
                let mut out = dst;
                src_used = self.Kraken_DecodeBytes(
                    &mut out,
                    src,
                    src_end,
                    &mut written_bytes,
                    dst_count,
                    false,
                    scratch,
                    scratch_end,
                );
                if src_used < 0 || written_bytes != dst_count {
                    panic!()
                }
            } else {
                src += 3;
                let src_used = chunkhdr & 0x7FFFF;
                let mode = (chunkhdr >> 19) & 0xF;
                if src_end - src < src_used {
                    panic!()
                }
                if src_used < dst_count {
                    let scratch_usage = std::cmp::min(
                        std::cmp::min(3 * dst_count + 32 + 0xd000, 0x6C000),
                        scratch_end - scratch,
                    );
                    if let Some(mut lz) = self.Kraken_ReadLzTable(
                        mode,
                        src,
                        src + src_used,
                        dst,
                        dst_count,
                        dst - dst_start,
                        scratch,
                        scratch + scratch_usage,
                    ) {
                        if !self.Kraken_ProcessLzRuns(
                            &mut lz,
                            mode,
                            dst,
                            dst_count,
                            dst - dst_start,
                        ) {
                            panic!()
                        }
                    } else {
                        panic!()
                    }
                } else if src_used > dst_count || mode != 0 {
                    panic!();
                } else {
                    self.memmove(dst, src, dst_count);
                }
            }
            src += src_used;
            dst += dst_count;
        }

        return src - src_in;
    }

    fn Kraken_DecodeBytes(
        &mut self,
        output: &mut Pointer,
        src: Pointer,
        src_end: Pointer,
        decoded_size: &mut usize,
        output_size: usize,
        force_memmove: bool,
        scratch: Pointer,
        scratch_end: Pointer,
    ) -> usize {
        let src_org = src;
        let src_size;
        let dst_size;

        if src_end - src < 2 {
            panic!()
        } // too few bytes

        let chunk_type = (self.get_as_usize(src + 0) >> 4) & 0x7;
        if chunk_type == 0 {
            if self.get_as_usize(src + 0) >= 0x80 {
                // In this mode, memcopy stores the length in the bottom 12 bits.
                src_size = ((self.get_as_usize(src + 0) << 8) | self.get_as_usize(src + 1)) & 0xFFF;
                src += 2;
            } else {
                if src_end - src < 3 {
                    panic!()
                } // too few bytes
                src_size = (self.get_as_usize(src + 0) << 16)
                    | (self.get_as_usize(src + 1) << 8)
                    | self.get_as_usize(src + 2);
                if (src_size & !0x3ffff) != 0 {
                    panic!()
                } // reserved bits must not be set
                src += 3;
            }
            if src_size > output_size || src_end - src < src_size {
                panic!()
            }
            *decoded_size = src_size;
            if force_memmove {
                self.memmove(*output, src, src_size);
            } else {
                *output = src;
            }
            return src + src_size - src_org;
        }

        // In all the other modes, the initial bytes encode
        // the src_size and the dst_size
        if self.get_as_usize(src + 0) >= 0x80 {
            if src_end - src < 3 {
                panic!()
            } // too few bytes

            // short mode, 10 bit sizes
            let bits = (self.get_as_usize(src + 0) << 16)
                | (self.get_as_usize(src + 1) << 8)
                | self.get_as_usize(src + 2);
            src_size = bits & 0x3ff;
            dst_size = src_size + ((bits >> 10) & 0x3ff) + 1;
            src += 3;
        } else {
            // long mode, 18 bit sizes
            if src_end - src < 5 {
                panic!()
            } // too few bytes
            let bits = (self.get_as_usize(src + 1) << 24)
                | (self.get_as_usize(src + 2) << 16)
                | (self.get_as_usize(src + 3) << 8)
                | self.get_as_usize(src + 4);
            src_size = bits & 0x3ffff;
            dst_size = (((bits >> 18) | (self.get_as_usize(src + 0) << 14)) & 0x3FFFF) + 1;
            if src_size >= dst_size {
                panic!()
            }
            src += 5;
        }
        if src_end - src < src_size || dst_size > output_size {
            panic!()
        }

        let dst = *output;
        if dst == scratch {
            if scratch_end - scratch < dst_size {
                panic!()
            }
            scratch += dst_size;
        }

        let src_used = match chunk_type {
            2 | 4 => Kraken_DecodeBytes_Type12(src, src_size, dst, dst_size, chunk_type >> 1),
            5 => Krak_DecodeRecursive(src, src_size, dst, dst_size, scratch, scratch_end),
            3 => Krak_DecodeRLE(src, src_size, dst, dst_size, scratch, scratch_end),
            1 => Krak_DecodeTans(src, src_size, dst, dst_size, scratch, scratch_end),
        };
        if src_used != src_size {
            panic!()
        }
        *decoded_size = dst_size;
        return src + src_size - src_org;
    }

    fn Kraken_ReadLzTable(
        &mut self,
        mode: usize,
        mut src: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
        mut scratch: Pointer,
        scratch_end: Pointer,
    ) -> Option<KrakenLzTable> {
        let mut out;
        let mut decode_count = 0;
        let mut n;
        let mut packed_offs_stream;
        let mut packed_len_stream;

        if mode > 1 {
            return None;
        }

        if src_end - src < 13 {
            return None;
        }

        if offset == 0 {
            self.memmove(dst, src, 8);
            dst += 8;
            src += 8;
        }

        if self.get_as_usize(src) & 0x80 != 0 {
            let flag = self.get_as_usize(src);
            src += 1;
            if (flag & 0xc0) != 0x80 {
                return None; // reserved flag set
            }

            return None; // excess bytes not supported
        }

        // Disable no copy optimization if source and dest overlap
        let force_copy = dst <= src_end && src <= dst + dst_size;

        // Decode lit stream, bounded by dst_size
        out = scratch;
        n = self.Kraken_DecodeBytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            std::cmp::min(scratch_end - scratch, dst_size),
            force_copy,
            scratch,
            scratch_end,
        );
        if n < 0 {
            return None;
        }
        src += n;
        let mut lz = KrakenLzTable {
            lit_stream: out,
            lit_stream_size: decode_count,
            ..Default::default()
        };
        scratch += decode_count;

        // Decode command stream, bounded by dst_size
        out = scratch;
        n = self.Kraken_DecodeBytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            std::cmp::min(scratch_end - scratch, dst_size),
            force_copy,
            scratch,
            scratch_end,
        );
        if n < 0 {
            return None;
        }
        src += n;
        lz.cmd_stream = out;
        lz.cmd_stream_size = decode_count;
        scratch += decode_count;

        // Check if to decode the multistuff crap
        if src_end - src < 3 {
            return None;
        }

        let mut offs_scaling = 0;
        let mut packed_offs_stream_extra = Default::default();

        if self.get_as_usize(src) & 0x80 != 0 {
            // uses the mode where distances are coded with 2 tables
            offs_scaling = i32::from(self.get_byte(src)) - 127;
            src += 1;

            packed_offs_stream = scratch;
            n = self.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lz.offs_stream_size,
                std::cmp::min(scratch_end - scratch, lz.cmd_stream_size),
                false,
                scratch,
                scratch_end,
            );
            if n < 0 {
                return None;
            }
            src += n;
            scratch += lz.offs_stream_size;

            if offs_scaling != 1 {
                packed_offs_stream_extra = scratch;
                n = self.Kraken_DecodeBytes(
                    &mut packed_offs_stream_extra,
                    src,
                    src_end,
                    &mut decode_count,
                    std::cmp::min(scratch_end - scratch, lz.offs_stream_size),
                    false,
                    scratch,
                    scratch_end,
                );
                if n < 0 || decode_count != lz.offs_stream_size {
                    return None;
                }
                src += n;
                scratch += decode_count;
            }
        } else {
            // Decode packed offset stream, it's bounded by the command length.
            packed_offs_stream = scratch;
            n = self.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lz.offs_stream_size,
                std::cmp::min(scratch_end - scratch, lz.cmd_stream_size),
                false,
                scratch,
                scratch_end,
            );
            if n < 0 {
                return None;
            }
            src += n;
            scratch += lz.offs_stream_size;
        }

        // Decode packed litlen stream. It's bounded by 1/4 of dst_size.
        packed_len_stream = scratch;
        n = self.Kraken_DecodeBytes(
            &mut packed_len_stream,
            src,
            src_end,
            &mut lz.len_stream_size,
            std::cmp::min(scratch_end - scratch, dst_size >> 2),
            false,
            scratch,
            scratch_end,
        );
        if n < 0 {
            return None;
        }
        src += n;
        scratch += lz.len_stream_size;

        // Reserve memory for final dist stream
        scratch = align_pointer(scratch, 16);
        lz.offs_stream = scratch.into();
        scratch += lz.offs_stream_size * 4;

        // Reserve memory for final len stream
        scratch = align_pointer(scratch, 16);
        lz.len_stream = scratch.into();
        scratch += lz.len_stream_size * 4;

        if scratch + 64 > scratch_end {
            return None;
        }

        let packed_offs_stream_size = lz.offs_stream_size;
        let packed_len_stream_size = lz.len_stream_size;
        if !self.Kraken_UnpackOffsets(
            &mut lz,
            src,
            src_end,
            packed_offs_stream,
            packed_offs_stream_extra,
            packed_offs_stream_size,
            offs_scaling,
            packed_len_stream,
            packed_len_stream_size,
            false,
            0,
        ) {
            return None;
        }

        return Some(lz);
    }

    // Unpacks the packed 8 bit offset and lengths into 32 bit.
    fn Kraken_UnpackOffsets(
        &mut self,
        lz: &mut KrakenLzTable,
        src: Pointer,
        src_end: Pointer,
        mut packed_offs_stream: Pointer,
        packed_offs_stream_extra: Pointer,
        packed_offs_stream_size: usize,
        multi_dist_scale: i32,
        packed_litlen_stream: Pointer,
        packed_litlen_stream_size: usize,
        excess_flag: bool,
        excess_bytes: i32,
    ) -> bool {
        let mut n;
        let mut i;
        let mut u32_len_stream_size = 0;

        let mut bits_a = BitReader {
            bitpos: 24,
            bits: 0,
            p: src,
            p_end: src_end,
        };
        bits_a.Refill(self);

        let mut bits_b = BitReader {
            bitpos: 24,
            bits: 0,
            p: src_end,
            p_end: src,
        };
        bits_b.RefillBackwards(self);

        if !excess_flag {
            if bits_b.bits < 0x2000 {
                return false;
            }
            n = bits_b.bits.ilog2();
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.RefillBackwards(self);
            n += 1;
            u32_len_stream_size = (bits_b.bits >> (32 - n)) - 1;
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.RefillBackwards(self);
        }

        if multi_dist_scale == 0 {
            // Traditional way of coding offsets
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            while packed_offs_stream != packed_offs_stream_end {
                self.set_int(
                    lz.offs_stream,
                    -i32::try_from(bits_a.ReadDistance(self, self.get_as_u32(packed_offs_stream)))
                        .unwrap(),
                );
                lz.offs_stream += 1;
                packed_offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                self.set_int(
                    lz.offs_stream,
                    -i32::try_from(bits_b.ReadDistanceB(self, self.get_as_u32(packed_offs_stream)))
                        .unwrap(),
                );
                lz.offs_stream += 1;
                packed_offs_stream += 1;
            }
        } else {
            // New way of coding offsets
            let mut offs_stream_org = lz.offs_stream;
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            let mut cmd;
            let mut offs;
            while packed_offs_stream != packed_offs_stream_end {
                cmd = i32::from(self.get_byte(packed_offs_stream));
                packed_offs_stream += 1;
                if (cmd >> 3) > 26 {
                    return false;
                }
                offs = ((8 + (cmd & 7)) << (cmd >> 3)) | bits_a.ReadMoreThan24Bits(self, cmd >> 3);
                self.set_int(lz.offs_stream, 8 - offs);
                lz.offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                cmd = self.get_as_u32(packed_offs_stream);
                packed_offs_stream += 1;
                if (cmd >> 3) > 26 {
                    return false;
                }
                offs = ((8 + (cmd & 7)) << (cmd >> 3))
                    | bits_b.ReadMoreThan24BitsB(&mut self, cmd >> 3);
                self.set_int(lz.offs_stream, 8 - offs.try_into().unwrap());
                lz.offs_stream += 1;
            }
            if multi_dist_scale != 1 {
                self.CombineScaledOffsetArrays(
                    &offs_stream_org,
                    lz.offs_stream - offs_stream_org,
                    multi_dist_scale,
                    &packed_offs_stream_extra,
                );
            }
        }
        let u32_len_stream_buf = [0u32; 512]; // max count is 128kb / 256 = 512
        if u32_len_stream_size > 512 {
            return false;
        }

        let mut u32_len_stream = 0;
        for i in 0usize..u32_len_stream_size.try_into().unwrap() {
            if i % 2 == 0 {
                if let Some(v) = bits_a.ReadLength(&mut self) {
                    u32_len_stream_buf[i] = v
                } else {
                    return false;
                }
            } else if let Some(v) = bits_b.ReadLengthB(&mut self) {
                u32_len_stream_buf[i] = v
            } else {
                return false;
            }
        }

        bits_a.p -= (24 - bits_a.bitpos.try_into().unwrap()) >> 3;
        bits_b.p += (24 - bits_b.bitpos.try_into().unwrap()) >> 3;

        if bits_a.p != bits_b.p {
            return false;
        }

        for i in 0..packed_litlen_stream_size {
            let v = self.get_as_u32(packed_litlen_stream + i);
            if v == 255 {
                v = u32_len_stream_buf[u32_len_stream] + 255;
                u32_len_stream += 1;
            }
            self.set_int(lz.len_stream + i, (v + 3).try_into().unwrap());
        }
        if u32_len_stream != u32_len_stream_buf.len() {
            return false;
        }

        return true;
    }

    fn CombineScaledOffsetArrays(
        &mut self,
        offs_stream: &IntPointer,
        offs_stream_size: usize,
        scale: i32,
        low_bits: &Pointer,
    ) {
        for i in 0..offs_stream_size {
            let scaled = scale * self.get_int(offs_stream + i) + self.get_byte(low_bits + i).into();
            self.set_int(offs_stream + i, scaled)
        }
    }

    fn Kraken_ProcessLzRuns(
        &mut self,
        lz: &mut KrakenLzTable,
        mode: usize,
        dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> bool {
        let dst_end = dst + dst_size;

        if mode == 1 {
            return self.Kraken_ProcessLzRuns_Type1(
                lz,
                dst + (if offset == 0 { 8 } else { 0 }),
                dst_end,
            );
        }

        if mode == 0 {
            return self.Kraken_ProcessLzRuns_Type0(
                lz,
                dst + (if offset == 0 { 8 } else { 0 }),
                dst_end,
            );
        }

        return false;
    }

    // Note: may access memory out of bounds on invalid input.
    fn Kraken_ProcessLzRuns_Type0(
        &mut self,
        lz: &mut KrakenLzTable,
        mut dst: Pointer,
        dst_end: Pointer,
    ) -> bool {
        let mut cmd_stream = lz.cmd_stream;
        let cmd_stream_end = cmd_stream + lz.cmd_stream_size;
        let mut len_stream = lz.len_stream;
        let len_stream_end = lz.len_stream + lz.len_stream_size;
        let mut lit_stream = lz.lit_stream;
        let lit_stream_end = lz.lit_stream + lz.lit_stream_size;
        let mut offs_stream = lz.offs_stream;
        let offs_stream_end = lz.offs_stream + lz.offs_stream_size;
        let mut copyfrom;
        let final_len;
        let mut offset;
        let mut recent_offs: [isize; 7] = [0; 7];
        let mut last_offset: isize;

        recent_offs[3] = -8;
        recent_offs[4] = -8;
        recent_offs[5] = -8;
        last_offset = -8;

        while cmd_stream < cmd_stream_end {
            let f = self.get_as_usize(cmd_stream);
            cmd_stream = cmd_stream + 1;
            let mut litlen = f & 3;
            let offs_index = f >> 6;
            let mut matchlen = (f >> 2) & 0xF;

            // use cmov
            let next_long_length = self.get_int(len_stream);
            let next_len_stream = len_stream + 1;

            len_stream = if litlen == 3 {
                next_len_stream
            } else {
                len_stream
            };
            litlen = if litlen == 3 {
                next_long_length.try_into().unwrap()
            } else {
                litlen
            };
            recent_offs[6] = self.get_int(offs_stream);

            self.copy_add(dst, lit_stream, dst + last_offset, litlen);
            dst += litlen;
            lit_stream += litlen;

            offset = recent_offs[offs_index + 3];
            recent_offs[offs_index + 3] = recent_offs[offs_index + 2];
            recent_offs[offs_index + 2] = recent_offs[offs_index + 1];
            recent_offs[offs_index + 1] = recent_offs[offs_index + 0];
            recent_offs[3] = offset;
            last_offset = offset;

            offs_stream = offs_stream.add_byte_offset((offs_index + 1) & 4);

            copyfrom = dst + offset;
            if matchlen != 15 {
                self.copy_64(dst, copyfrom);
                self.copy_64(dst + 8, copyfrom + 8);
                dst += matchlen + 2;
            } else {
                // why is the value not 16 here, the above case copies up to 16 bytes.
                matchlen = (14 + self.get_int(len_stream)).try_into().unwrap();
                len_stream += 1;
                self.memmove(dst, copyfrom, matchlen);
                dst += matchlen;
            }
        }

        // check for incorrect input
        if offs_stream != offs_stream_end || len_stream != len_stream_end {
            return false;
        }

        final_len = dst_end - dst;
        if final_len != lit_stream_end - lit_stream {
            return false;
        }

        self.copy_add(dst, lit_stream, dst + last_offset, final_len);
        return true;
    }

    // Note: may access memory out of bounds on invalid input.
    fn Kraken_ProcessLzRuns_Type1(
        &mut self,
        lz: &mut KrakenLzTable,
        mut dst: Pointer,
        dst_end: Pointer,
    ) -> bool {
        let mut cmd_stream = lz.cmd_stream;
        let cmd_stream_end = cmd_stream + lz.cmd_stream_size;
        let mut len_stream = lz.len_stream;
        let len_stream_end = lz.len_stream + lz.len_stream_size;
        let mut lit_stream = lz.lit_stream;
        let lit_stream_end = lz.lit_stream + lz.lit_stream_size;
        let mut offs_stream = lz.offs_stream;
        let offs_stream_end = lz.offs_stream + lz.offs_stream_size;
        let mut copyfrom;
        let final_len;
        let mut offset;
        let mut recent_offs = [0; 7];

        recent_offs[3] = -8;
        recent_offs[4] = -8;
        recent_offs[5] = -8;

        while cmd_stream < cmd_stream_end {
            let f = self.get_as_usize(cmd_stream);
            cmd_stream = cmd_stream + 1;
            let mut litlen = f & 3;
            let offs_index = f >> 6;
            let mut matchlen = (f >> 2) & 0xF;

            // use cmov
            let next_long_length = self.get_int(len_stream);
            let next_len_stream = len_stream + 1;

            len_stream = if litlen == 3 {
                next_len_stream
            } else {
                len_stream
            };
            litlen = if litlen == 3 {
                next_long_length.try_into().unwrap()
            } else {
                litlen
            };
            recent_offs[6] = self.get_int(offs_stream);

            self.memmove(dst, lit_stream, litlen);
            dst += litlen;
            lit_stream += litlen;

            offset = recent_offs[offs_index + 3];
            recent_offs[offs_index + 3] = recent_offs[offs_index + 2];
            recent_offs[offs_index + 2] = recent_offs[offs_index + 1];
            recent_offs[offs_index + 1] = recent_offs[offs_index + 0];
            recent_offs[3] = offset;

            offs_stream = offs_stream.add_byte_offset((offs_index + 1) & 4);

            copyfrom = dst + offset;
            if matchlen != 15 {
                self.memmove(dst, copyfrom, matchlen + 2);
                dst += matchlen + 2;
            } else {
                // why is the value not 16 here, the above case copies up to 16 bytes.
                matchlen = (14 + self.get_int(len_stream)).try_into().unwrap();
                len_stream = len_stream + 1;
                self.memmove(dst, copyfrom, matchlen);
                dst += matchlen;
            }
        }

        // check for incorrect input
        if offs_stream != offs_stream_end || len_stream != len_stream_end {
            return false;
        }

        final_len = dst_end - dst;
        if final_len != lit_stream_end - lit_stream {
            return false;
        }

        self.memmove(dst, lit_stream, final_len);
        return true;
    }
}

impl KrakenDecoder {
    fn get_byte(&self, p: Pointer) -> u8 {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input[p.index],
            PointerDest::Output => self.output[p.index],
            PointerDest::Scratch => self.scratch[p.index],
        }
    }
    fn get_as_usize(&self, p: Pointer) -> usize {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => usize::from(self.input[p.index]),
            PointerDest::Output => usize::from(self.output[p.index]),
            PointerDest::Scratch => usize::from(self.scratch[p.index]),
        }
    }

    fn get_as_u32(&self, p: Pointer) -> u32 {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => u32::from(self.input[p.index]),
            PointerDest::Output => u32::from(self.output[p.index]),
            PointerDest::Scratch => u32::from(self.scratch[p.index]),
        }
    }

    fn get_int(&self, p: IntPointer) -> i32 {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => {
                i32::from_le_bytes(self.input[p.index..p.index + 4].try_into().unwrap())
            }
            PointerDest::Output => {
                i32::from_le_bytes(self.output[p.index..p.index + 4].try_into().unwrap())
            }
            PointerDest::Scratch => {
                i32::from_le_bytes(self.scratch[p.index..p.index + 4].try_into().unwrap())
            }
        }
    }

    fn set(&mut self, p: Pointer, v: u8) {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input[p.index] = v,
            PointerDest::Output => self.output[p.index] = v,
            PointerDest::Scratch => self.scratch[p.index] = v,
        }
    }

    fn set_int(&mut self, p: IntPointer, v: i32) {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => {
                self.input[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
            PointerDest::Output => {
                self.output[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
            PointerDest::Scratch => {
                self.scratch[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
        }
    }

    fn copy_64(&mut self, dest: Pointer, src: Pointer) {
        self.memmove(dest, src, 8)
    }

    fn copy_64_bytes(&mut self, dest: Pointer, src: Pointer) {
        self.memmove(dest, src, 64)
    }

    fn copy_add(&mut self, dst: Pointer, lhs: Pointer, rhs: Pointer, n: usize) {
        for i in 0..n {
            self.set(dst + i, self.get_byte(lhs) + self.get_byte(rhs))
        }
    }

    fn memmove(&mut self, dest: Pointer, src: Pointer, n: usize) {
        if dest.into == src.into {
            if dest.index != src.index {
                match dest.into {
                    PointerDest::Null => panic!(),
                    PointerDest::Input => &mut self.input,
                    PointerDest::Output => &mut self.output,
                    PointerDest::Scratch => &mut self.scratch,
                }
                .copy_within(src.index..src.index + n, dest.index)
            }
        } else {
            match dest.into {
                PointerDest::Null => panic!(),
                PointerDest::Input => {
                    self.input[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => panic!(),
                        PointerDest::Output => &self.output[src.index..src.index + n],
                        PointerDest::Scratch => &self.scratch[src.index..src.index + n],
                    })
                }
                PointerDest::Output => {
                    self.output[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => &self.input[src.index..src.index + n],
                        PointerDest::Output => panic!(),
                        PointerDest::Scratch => &self.scratch[src.index..src.index + n],
                    })
                }
                PointerDest::Scratch => {
                    self.scratch[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => &self.input[src.index..src.index + n],
                        PointerDest::Output => &self.output[src.index..src.index + n],
                        PointerDest::Scratch => panic!(),
                    })
                }
            }
        }
    }
}

fn align_16(x: usize) -> usize {
    (x + 15) & !15
}

fn align_pointer(p: Pointer, align: usize) -> Pointer {
    Pointer {
        index: (p.index + (align - 1)) & !(align - 1),
        ..p
    }
}
