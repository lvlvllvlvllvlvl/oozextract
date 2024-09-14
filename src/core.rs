use crate::algorithm::Algorithm;
use crate::bit_reader::{BitReader, BitReader2};
use crate::huffman::{HuffRange, HuffReader, HuffRevLut, BASE_PREFIX};
use crate::pointer::{IntPointer, Pointer, PointerDest};
use crate::tans::TansDecoder;
use std::fmt::Debug;

pub struct Core<'a> {
    pub input: &'a [u8],
    pub output: &'a mut [u8],
    pub scratch: Vec<u8>,
}

impl Core<'_> {
    // Decode one 256kb big quantum block. It's divided into two 128k blocks
    // internally that are compressed separately but with a shared history.
    pub fn decode_quantum<T: Algorithm + Debug>(
        &mut self,
        write_from: usize,
        write_count: usize,
        algorithm: T,
    ) -> usize {
        let mut written_bytes = 0;
        let mut src = Pointer::input(0);
        let src_in = Pointer::input(0);
        let src_end = Pointer::input(self.input.len());
        let mut dst = Pointer::output(write_from);
        let dst_start = Pointer::output(0);
        let dst_end = Pointer::output(write_from + write_count);
        let mut src_used;

        while dst_end > dst {
            let dst_count = std::cmp::min(dst_end - dst, 0x20000);
            assert!(src_end - src >= 4, "{:?} {:?}", src_end, src);
            let chunkhdr = self.get_bytes_as_usize_be(src, 3);
            log::debug!("index: {}, chunk header: {}", src - src_in, chunkhdr);
            if (chunkhdr & 0x800000) == 0 {
                log::debug!("Stored as entropy without any match copying.");
                let mut out = dst;
                src_used = self.Kraken_DecodeBytes(
                    &mut out,
                    src,
                    src_end,
                    &mut written_bytes,
                    dst_count,
                    false,
                    Pointer::scratch(0),
                );
                assert_eq!(written_bytes, dst_count);
            } else {
                src += 3;
                src_used = chunkhdr & 0x7FFFF;
                let mode = (chunkhdr >> 19) & 0xF;
                assert!(src_end - src >= src_used, "{} {}", src_end - src, src_used);
                if src_used < dst_count {
                    log::debug!("processing with {:?}", algorithm);
                    algorithm.process(self, mode, src, src_used, dst_start, dst, dst_count);
                } else if src_used > dst_count || mode != 0 {
                    panic!();
                } else {
                    log::debug!("copying {} bytes", dst_count);
                    self.memmove(dst, src, dst_count);
                }
            }
            src += src_used;
            dst += dst_count;
        }

        src - src_in
    }

    // Unpacks the packed 8 bit offset and lengths into 32 bit.
    pub fn Kraken_UnpackOffsets(
        &mut self,
        src: Pointer,
        src_end: Pointer,
        mut packed_offs_stream: Pointer,
        packed_offs_stream_extra: Pointer,
        packed_offs_stream_size: usize,
        multi_dist_scale: i32,
        packed_litlen_stream: Pointer,
        packed_litlen_stream_size: usize,
        mut offs_stream: IntPointer,
        len_stream: IntPointer,
        excess_flag: bool,
    ) {
        let mut n;
        let mut u32_len_stream_size = 0usize;
        let offs_stream_org = offs_stream;

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
            assert!(bits_b.bits >= 0x2000, "{:X}", bits_b.bits);
            n = bits_b.leading_zeros();
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.RefillBackwards(self);
            n += 1;
            u32_len_stream_size = ((bits_b.bits >> (32 - n)) - 1) as usize;
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.RefillBackwards(self);
        }

        if multi_dist_scale == 0 {
            // Traditional way of coding offsets
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            while packed_offs_stream != packed_offs_stream_end {
                let d_a = bits_a.ReadDistance(self, self.get_byte(packed_offs_stream).into());
                self.set_int(offs_stream, -d_a);
                offs_stream += 1;
                packed_offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                let d_b = bits_b.ReadDistanceB(self, self.get_byte(packed_offs_stream).into());
                self.set_int(offs_stream, -d_b);
                offs_stream += 1;
                packed_offs_stream += 1;
            }
        } else {
            // New way of coding offsets
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            let mut cmd;
            let mut offs;
            while packed_offs_stream != packed_offs_stream_end {
                cmd = i32::from(self.get_byte(packed_offs_stream));
                packed_offs_stream += 1;
                assert!((cmd >> 3) <= 26, "{}", cmd >> 3);
                offs = ((8 + (cmd & 7)) << (cmd >> 3)) | bits_a.ReadMoreThan24Bits(self, cmd >> 3);
                self.set_int(offs_stream, 8 - offs);
                offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                cmd = i32::from(self.get_byte(packed_offs_stream));
                packed_offs_stream += 1;
                assert!((cmd >> 3) <= 26, "{}", cmd >> 3);
                offs = ((8 + (cmd & 7)) << (cmd >> 3)) | bits_b.ReadMoreThan24BitsB(self, cmd >> 3);
                self.set_int(offs_stream, 8 - offs);
                offs_stream += 1;
            }
            if multi_dist_scale != 1 {
                self.CombineScaledOffsetArrays(
                    &offs_stream_org,
                    offs_stream - offs_stream_org,
                    multi_dist_scale,
                    &packed_offs_stream_extra,
                );
            }
        }
        let mut u32_len_stream_buf = [0u32; 512]; // max count is 128kb / 256 = 512
        assert!(u32_len_stream_size <= 512, "{:?}", u32_len_stream_size);

        let mut u32_len_stream = 0;
        for (i, dst) in u32_len_stream_buf[..u32_len_stream_size]
            .iter_mut()
            .enumerate()
        {
            if i % 2 == 0 {
                *dst = bits_a.ReadLength(self) as u32
            } else {
                *dst = bits_b.ReadLengthB(self) as u32
            }
        }

        bits_a.p -= (24 - bits_a.bitpos) >> 3;
        bits_b.p += (24 - bits_b.bitpos) >> 3;

        assert_eq!(bits_a.p, bits_b.p);

        for i in 0..packed_litlen_stream_size {
            let mut v = u32::from(self.get_byte(packed_litlen_stream + i));
            if v == 255 {
                v = u32_len_stream_buf[u32_len_stream] + 255;
                u32_len_stream += 1;
            }
            self.set_int(len_stream + i, (v + 3) as i32);
        }
        assert_eq!(u32_len_stream, u32_len_stream_size);
    }

    fn CombineScaledOffsetArrays(
        &mut self,
        offs_stream: &IntPointer,
        offs_stream_size: usize,
        scale: i32,
        low_bits: &Pointer,
    ) {
        for i in 0..offs_stream_size {
            let scaled =
                scale * self.get_int(offs_stream + i) + i32::from(self.get_byte(low_bits + i));
            self.set_int(offs_stream + i, scaled)
        }
    }

    pub fn Kraken_DecodeBytes(
        &mut self,
        output: &mut Pointer,
        mut src: Pointer,
        src_end: Pointer,
        decoded_size: &mut usize,
        output_size: usize,
        force_memmove: bool,
        mut scratch: Pointer,
    ) -> usize {
        let src_org = src;
        let src_size;
        let dst_size;

        assert!(src_end - src >= 2, "too few bytes {}", src_end - src);

        let chunk_type = (self.get_as_usize(src + 0) >> 4) & 0x7;
        if chunk_type == 0 {
            if self.get_as_usize(src + 0) >= 0x80 {
                // In this mode, memcopy stores the length in the bottom 12 bits.
                src_size = ((self.get_as_usize(src + 0) << 8) | self.get_as_usize(src + 1)) & 0xFFF;
                src += 2;
            } else {
                assert!(src_end - src >= 3, "too few bytes {}", src_end - src);
                src_size = self.get_bytes_as_usize_be(src, 3);
                assert_eq!(
                    src_size & !0x3ffff,
                    0,
                    "reserved bits must not be set {:X}",
                    src_size & !0x3ffff
                );
                src += 3;
            }
            assert!(src_size <= output_size, "{} {}", src_size, output_size);
            assert!(src_size <= src_end - src, "{} {}", src_end - src, src_size);
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
            assert!(src_end - src >= 3, "too few bytes {}", src_end - src);

            // short mode, 10 bit sizes
            let bits = self.get_bytes_as_usize_be(src, 3);
            src_size = bits & 0x3ff;
            dst_size = src_size + ((bits >> 10) & 0x3ff) + 1;
            src += 3;
        } else {
            // long mode, 18 bit sizes
            assert!(src_end - src >= 5, "too few bytes {}", src_end - src);
            let bits = self.get_bytes_as_usize_be(src + 1, 4);
            src_size = bits & 0x3ffff;
            dst_size = (((bits >> 18) | (self.get_as_usize(src + 0) << 14)) & 0x3FFFF) + 1;
            assert!(src_size < dst_size, "{} {}", src_size, dst_size);
            src += 5;
        }
        assert!(src_size <= src_end - src, "{} {}", src_size, src_end - src);
        assert!(dst_size <= output_size, "{} {}", dst_size, output_size);

        let dst = *output;
        if dst.into == PointerDest::Scratch {
            scratch += dst_size;
        }

        let src_used = match chunk_type {
            2 | 4 => {
                Some(self.Kraken_DecodeBytes_Type12(src, src_size, dst, dst_size, chunk_type >> 1))
            }
            5 => self.Krak_DecodeRecursive(src, src_size, dst, dst_size, scratch),
            3 => self.Krak_DecodeRLE(src, src_size, dst, dst_size, scratch),
            1 => self.Krak_DecodeTans(src, src_size, dst, dst_size),
            _ => panic!("{}", chunk_type),
        };
        assert!(
            src_used.is_some_and(|used| used == src_size),
            "{:?} {} ({})",
            src_used,
            src_size,
            chunk_type
        );
        *decoded_size = dst_size;
        src + src_size - src_org
    }

    fn Kraken_DecodeBytes_Type12(
        &mut self,
        mut src: Pointer,
        src_size: usize,
        output: Pointer,
        output_size: usize,
        chunk_type: usize,
    ) -> usize {
        let half_output_size;
        let split_left;
        let split_mid;
        let split_right;
        let src_mid;
        let src_end = src + src_size;

        let mut bits = BitReader {
            bitpos: 24,
            bits: 0,
            p: src,
            p_end: src_end,
        };
        bits.Refill(self);

        let mut code_prefix = BASE_PREFIX;
        let mut syms = [0; 1280];
        let num_syms;
        if !bits.ReadBitNoRefill() {
            num_syms = self.Huff_ReadCodeLengthsOld(&mut bits, &mut syms, &mut code_prefix);
        } else if !bits.ReadBitNoRefill() {
            num_syms = self
                .Huff_ReadCodeLengthsNew(&mut bits, &mut syms, &mut code_prefix)
                .unwrap();
        } else {
            panic!();
        }
        src = bits.p - ((24 - bits.bitpos) / 8);

        if num_syms == 1 {
            self.memset(output, syms[0], output_size);
            return src - src_end;
        }

        let rev_lut = HuffRevLut::make_lut(&code_prefix, &syms).unwrap();

        if chunk_type == 1 {
            assert!(src + 3 <= src_end, "{:?} {:?}", src, src_end);
            split_mid = self.get_bytes_as_usize_le(src, 2);
            src += 2;
            let mut hr = HuffReader {
                output,
                output_end: output + output_size,
                src,
                src_end,
                src_mid_org: src + split_mid,
                src_mid: src + split_mid,
                ..Default::default()
            };
            hr.decode_bytes(self, &rev_lut);
        } else {
            assert!(src + 6 <= src_end, "{:?} {:?}", src, src_end);

            half_output_size = (output_size + 1) >> 1;
            split_mid = self.get_bytes_as_usize_le(src, 3);
            src += 3;
            assert!(
                split_mid <= src_end - src,
                "{} {}",
                split_mid,
                src_end - src
            );
            src_mid = src + split_mid;
            split_left = self.get_bytes_as_usize_le(src, 2);
            src += 2;
            assert!(
                split_left + 2 <= src_end - src,
                "{} {}",
                split_left + 2,
                src_end - src
            );
            assert!(src_end - src_mid >= 3, "{}", src_end - src_mid);
            split_right = self.get_bytes_as_usize_le(src_mid, 2);
            assert!(src_end - (src_mid + 2) >= split_right + 2);

            let mut hr = HuffReader {
                output,
                output_end: output + half_output_size,
                src,
                src_end: src_mid,
                src_mid_org: src + split_left,
                src_mid: src + split_left,
                ..Default::default()
            };
            hr.decode_bytes(self, &rev_lut);

            let mut hr = HuffReader {
                output: output + half_output_size,
                output_end: output + output_size,
                src: src_mid + 2,
                src_end,
                src_mid_org: src_mid + 2 + split_right,
                src_mid: src_mid + 2 + split_right,
                ..Default::default()
            };
            hr.decode_bytes(self, &rev_lut);
        }
        src_size
    }

    fn Huff_ReadCodeLengthsOld(
        &mut self,
        bits: &mut BitReader,
        syms: &mut [u8; 1280],
        code_prefix: &mut [usize; 12],
    ) -> i32 {
        if bits.ReadBitNoRefill() {
            let mut sym = 0;
            let mut num_symbols = 0;
            let mut avg_bits_x4 = 32;
            let forced_bits = bits.ReadBitsNoRefill(2);

            let thres_for_valid_gamma_bits = 1 << (31 - (20 >> forced_bits));
            let mut skip_initial_zeros = bits.ReadBit(self);
            while sym != 256 {
                if skip_initial_zeros {
                    skip_initial_zeros = false;
                } else {
                    // Run of zeros
                    assert_ne!(bits.bits & 0xff000000, 0);
                    sym += bits.ReadBitsNoRefill(2 * (bits.leading_zeros() + 1)) - 2 + 1;
                    if sym >= 256 {
                        break;
                    }
                }
                bits.Refill(self);
                // Read out the gamma value for the # of symbols
                assert_ne!(bits.bits & 0xff000000, 0);
                let mut n = bits.ReadBitsNoRefill(2 * (bits.leading_zeros() + 1)) - 2 + 1;
                assert!(sym + n <= 256, "Overflow? {} {}", sym, n);
                bits.Refill(self);
                num_symbols += n;
                loop {
                    assert!(
                        bits.bits >= thres_for_valid_gamma_bits,
                        "too big gamma value? {}, {}",
                        bits.bits,
                        thres_for_valid_gamma_bits
                    );

                    let lz = bits.leading_zeros();
                    let v = bits.ReadBitsNoRefill(lz + forced_bits + 1) + ((lz - 1) << forced_bits);
                    let codelen = (-(v & 1) ^ (v >> 1)) + ((avg_bits_x4 + 2) >> 2);
                    assert!(codelen >= 1, "{}", codelen);
                    assert!(codelen <= 11, "{}", codelen);
                    avg_bits_x4 = codelen + ((3 * avg_bits_x4 + 2) >> 2);
                    bits.Refill(self);
                    syms[code_prefix[usize::try_from(codelen).unwrap()]] = sym as _;
                    code_prefix[usize::try_from(codelen).unwrap()] += 1;
                    sym += 1;
                    n -= 1;
                    if n == 0 {
                        break;
                    }
                }
            }
            assert_eq!(sym, 256);
            assert!(num_symbols >= 2, "{}", num_symbols);
            num_symbols
        } else {
            // Sparse symbol encoding
            let num_symbols = bits.ReadBitsNoRefill(8);
            assert_ne!(num_symbols, 0);
            if num_symbols == 1 {
                syms[0] = bits.ReadBitsNoRefill(8) as _;
            } else {
                let codelen_bits = bits.ReadBitsNoRefill(3);
                assert!(codelen_bits <= 4, "{}", codelen_bits);
                for _ in 0..num_symbols {
                    bits.Refill(self);
                    let sym = bits.ReadBitsNoRefill(8) as u8;
                    let codelen = bits.ReadBitsNoRefillZero(codelen_bits) + 1;
                    assert!(codelen <= 11, "{}", codelen);
                    syms[code_prefix[usize::try_from(codelen).unwrap()]] = sym;
                    code_prefix[usize::try_from(codelen).unwrap()] += 1;
                }
            }
            num_symbols
        }
    }

    fn Huff_ReadCodeLengthsNew(
        &mut self,
        bits: &mut BitReader,
        syms: &mut [u8; 1280],
        code_prefix: &mut [usize; 12],
    ) -> Option<i32> {
        let forced_bits = bits.ReadBitsNoRefill(2);

        let num_symbols = bits.ReadBitsNoRefill(8) + 1;

        let fluff = bits.ReadFluff(num_symbols);

        let mut code_len = [0; 512];
        let mut br2 = BitReader2 {
            bitpos: ((bits.bitpos - 24) & 7) as u32,
            p_end: bits.p_end,
            p: bits.p - ((24 - bits.bitpos + 7) >> 3) as u32,
        };

        if !self.DecodeGolombRiceLengths(&mut code_len[..num_symbols as usize + fluff], &mut br2) {
            return None;
        }
        if !self.DecodeGolombRiceBits(
            &mut code_len[..usize::try_from(num_symbols).unwrap()],
            forced_bits as u32,
            &mut br2,
        ) {
            return None;
        }

        // Reset the bits decoder.
        bits.bitpos = 24;
        bits.p = br2.p;
        bits.bits = 0;
        bits.Refill(self);
        bits.bits <<= br2.bitpos;
        bits.bitpos += br2.bitpos as i32;

        let mut running_sum = 0x1e;
        for len in code_len.iter_mut() {
            let mut v = *len;
            v = (!(v & 1) + 1) ^ (v >> 1);
            *len = v + (running_sum >> 2) + 1;
            if *len < 1 || *len > 11 {
                return None;
            }
            running_sum += v;
        }

        let ranges = self.Huff_ConvertToRanges(num_symbols as u16, fluff, &code_len, bits)?;

        let mut cp = 0;
        for range in ranges {
            let mut sym = range.symbol;
            for i in &code_len[cp..][..range.num as usize] {
                syms[code_prefix[*i as usize]] = sym as u8;
                code_prefix[*i as usize] += 1;
                sym += 1;
            }
            cp += range.num as usize;
        }

        Some(num_symbols)
    }

    pub fn DecodeGolombRiceLengths(&self, mut dst: &mut [u8], br: &mut BitReader2) -> bool {
        const K_RICE_CODE_BITS2VALUE: [u32; 256] = [
            0x80000000, 0x00000007, 0x10000006, 0x00000006, 0x20000005, 0x00000105, 0x10000005,
            0x00000005, 0x30000004, 0x00000204, 0x10000104, 0x00000104, 0x20000004, 0x00010004,
            0x10000004, 0x00000004, 0x40000003, 0x00000303, 0x10000203, 0x00000203, 0x20000103,
            0x00010103, 0x10000103, 0x00000103, 0x30000003, 0x00020003, 0x10010003, 0x00010003,
            0x20000003, 0x01000003, 0x10000003, 0x00000003, 0x50000002, 0x00000402, 0x10000302,
            0x00000302, 0x20000202, 0x00010202, 0x10000202, 0x00000202, 0x30000102, 0x00020102,
            0x10010102, 0x00010102, 0x20000102, 0x01000102, 0x10000102, 0x00000102, 0x40000002,
            0x00030002, 0x10020002, 0x00020002, 0x20010002, 0x01010002, 0x10010002, 0x00010002,
            0x30000002, 0x02000002, 0x11000002, 0x01000002, 0x20000002, 0x00000012, 0x10000002,
            0x00000002, 0x60000001, 0x00000501, 0x10000401, 0x00000401, 0x20000301, 0x00010301,
            0x10000301, 0x00000301, 0x30000201, 0x00020201, 0x10010201, 0x00010201, 0x20000201,
            0x01000201, 0x10000201, 0x00000201, 0x40000101, 0x00030101, 0x10020101, 0x00020101,
            0x20010101, 0x01010101, 0x10010101, 0x00010101, 0x30000101, 0x02000101, 0x11000101,
            0x01000101, 0x20000101, 0x00000111, 0x10000101, 0x00000101, 0x50000001, 0x00040001,
            0x10030001, 0x00030001, 0x20020001, 0x01020001, 0x10020001, 0x00020001, 0x30010001,
            0x02010001, 0x11010001, 0x01010001, 0x20010001, 0x00010011, 0x10010001, 0x00010001,
            0x40000001, 0x03000001, 0x12000001, 0x02000001, 0x21000001, 0x01000011, 0x11000001,
            0x01000001, 0x30000001, 0x00000021, 0x10000011, 0x00000011, 0x20000001, 0x00001001,
            0x10000001, 0x00000001, 0x70000000, 0x00000600, 0x10000500, 0x00000500, 0x20000400,
            0x00010400, 0x10000400, 0x00000400, 0x30000300, 0x00020300, 0x10010300, 0x00010300,
            0x20000300, 0x01000300, 0x10000300, 0x00000300, 0x40000200, 0x00030200, 0x10020200,
            0x00020200, 0x20010200, 0x01010200, 0x10010200, 0x00010200, 0x30000200, 0x02000200,
            0x11000200, 0x01000200, 0x20000200, 0x00000210, 0x10000200, 0x00000200, 0x50000100,
            0x00040100, 0x10030100, 0x00030100, 0x20020100, 0x01020100, 0x10020100, 0x00020100,
            0x30010100, 0x02010100, 0x11010100, 0x01010100, 0x20010100, 0x00010110, 0x10010100,
            0x00010100, 0x40000100, 0x03000100, 0x12000100, 0x02000100, 0x21000100, 0x01000110,
            0x11000100, 0x01000100, 0x30000100, 0x00000120, 0x10000110, 0x00000110, 0x20000100,
            0x00001100, 0x10000100, 0x00000100, 0x60000000, 0x00050000, 0x10040000, 0x00040000,
            0x20030000, 0x01030000, 0x10030000, 0x00030000, 0x30020000, 0x02020000, 0x11020000,
            0x01020000, 0x20020000, 0x00020010, 0x10020000, 0x00020000, 0x40010000, 0x03010000,
            0x12010000, 0x02010000, 0x21010000, 0x01010010, 0x11010000, 0x01010000, 0x30010000,
            0x00010020, 0x10010010, 0x00010010, 0x20010000, 0x00011000, 0x10010000, 0x00010000,
            0x50000000, 0x04000000, 0x13000000, 0x03000000, 0x22000000, 0x02000010, 0x12000000,
            0x02000000, 0x31000000, 0x01000020, 0x11000010, 0x01000010, 0x21000000, 0x01001000,
            0x11000000, 0x01000000, 0x40000000, 0x00000030, 0x10000020, 0x00000020, 0x20000010,
            0x00001010, 0x10000010, 0x00000010, 0x30000000, 0x00002000, 0x10001000, 0x00001000,
            0x20000000, 0x00100000, 0x10000000, 0x00000000,
        ];

        const K_RICE_CODE_BITS2LEN: [u8; 256] = [
            0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4, 1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3,
            4, 4, 5, 1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4,
            4, 5, 4, 5, 5, 6, 1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2, 3, 3, 4, 3, 4, 4,
            5, 3, 4, 4, 5, 4, 5, 5, 6, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6, 3, 4, 4, 5,
            4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7, 1, 2, 2, 3, 2, 3, 3, 4, 2, 3, 3, 4, 3, 4, 4, 5, 2,
            3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5, 5, 6, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4, 5, 4, 5,
            5, 6, 3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7, 2, 3, 3, 4, 3, 4, 4, 5, 3, 4, 4,
            5, 4, 5, 5, 6, 3, 4, 4, 5, 4, 5, 5, 6, 4, 5, 5, 6, 5, 6, 6, 7, 3, 4, 4, 5, 4, 5, 5, 6,
            4, 5, 5, 6, 5, 6, 6, 7, 4, 5, 5, 6, 5, 6, 6, 7, 5, 6, 6, 7, 6, 7, 7, 8,
        ];

        let mut p = br.p;
        let p_end = br.p_end;
        if p >= p_end {
            return false;
        }

        let mut count = -(br.bitpos as i32);
        let mut v = self.get_as_usize(p) & (255 >> br.bitpos);
        p += 1;
        loop {
            if v == 0 {
                count += 8;
            } else {
                let x = K_RICE_CODE_BITS2VALUE[v] as i32;
                let bytes = [
                    (count + (x & 0x0f0f0f0f)).to_le_bytes(),
                    ((x >> 4) & 0x0f0f0f0f).to_le_bytes(),
                ]
                .concat();
                if bytes.len() > dst.len() {
                    dst.copy_from_slice(&bytes[..dst.len()]);
                } else {
                    dst[..8].copy_from_slice(&bytes);
                }
                let step = K_RICE_CODE_BITS2LEN[v] as usize;
                if dst.len() >= step {
                    // went too far, step back
                    for _ in dst.len()..step {
                        v &= v - 1;
                    }
                    break;
                }
                dst = &mut dst[step..];
                count = x >> 28;
            }
            if p >= p_end {
                return false;
            }
            v = self.get_byte(p) as _;
            p += 1;
        }
        // step back if byte not finished
        let mut bitpos = 0;
        if (v & 1) != 0 {
            p -= 1;
            bitpos = 8 - v.trailing_zeros();
        }
        br.p = p;
        br.bitpos = bitpos;
        true
    }

    fn DecodeGolombRiceBits(
        &mut self,
        mut dst: &mut [u8],
        bitcount: u32,
        br: &mut BitReader2,
    ) -> bool {
        if bitcount == 0 {
            return true;
        }
        let mut p = br.p;
        let bitpos = br.bitpos;

        let bits_required = ((bitpos + bitcount) as usize) * dst.len();
        let bytes_required = (bits_required + 7) >> 3;
        if bytes_required > br.p_end - p {
            return false;
        }

        br.p = p + (bits_required >> 3);
        br.bitpos = (bits_required & 7) as u32;

        while !dst.is_empty() {
            let bits = match bitcount {
                1 => {
                    // Read the next byte
                    let mut bits =
                        ((self.get_bytes_as_usize_be(p, 4) >> (24 - bitpos)) & 0xFF) as u64;
                    p += 1;
                    // Expand each bit into each byte of the uint64.
                    bits = (bits | (bits << 28)) & 0xF0000000F;
                    bits = (bits | (bits << 14)) & 0x3000300030003;
                    bits = (bits | (bits << 7)) & 0x0101010101010101;
                    bits
                }
                2 => {
                    // Read the next 2 bytes
                    let mut bits =
                        ((self.get_bytes_as_usize_be(p, 4) >> (16 - bitpos)) & 0xFFFF) as u64;
                    p += 2;
                    // Expand each bit into each byte of the uint64.
                    bits = (bits | (bits << 24)) & 0xFF000000FF;
                    bits = (bits | (bits << 12)) & 0xF000F000F000F;
                    bits = (bits | (bits << 6)) & 0x0303030303030303;
                    bits
                }
                3 => {
                    // Read the next 3 bytes
                    let mut bits =
                        ((self.get_bytes_as_usize_be(p, 4) >> (8 - bitpos)) & 0xffffff) as u64;
                    p += 3;
                    // Expand each bit into each byte of the uint64.
                    bits = (bits | (bits << 20)) & 0xFFF00000FFF;
                    bits = (bits | (bits << 10)) & 0x3F003F003F003F;
                    bits = (bits | (bits << 5)) & 0x0707070707070707;
                    bits
                }
                _ => panic!(),
            };
            let len = dst.len().max(8);
            let mut bytes = dst[..len].to_vec();
            bytes.resize(8, 0);
            let v = (u64::from_le_bytes(bytes.try_into().unwrap()) << bitcount) + bits.swap_bytes();
            dst.copy_from_slice(&v.to_le_bytes()[..len]);
            dst = &mut dst[len..];
        }
        true
    }

    pub fn Huff_ConvertToRanges(
        &self,
        num_symbols: u16,
        p: usize,
        symlen: &[u8],
        bits: &mut BitReader,
    ) -> Option<Vec<HuffRange>> {
        let mut symbol = 0;
        let mut idx = 0;

        // Start with space?
        if p & 1 != 0 {
            bits.Refill(self);
            let v = symlen[idx] as i32;
            idx += 1;
            if v >= 8 {
                return None;
            }
            symbol = u16::try_from(bits.ReadBitsNoRefill(v + 1) + (1 << (v + 1)) - 1).unwrap();
        }

        let mut syms_used = 0;
        let num_ranges = p >> 1;
        let mut ranges: Vec<HuffRange> = Vec::with_capacity(num_ranges + 1);

        for _ in 0..num_ranges {
            bits.Refill(self);
            let v = symlen[idx] as i32;
            idx += 1;
            if v >= 9 {
                return None;
            }
            let num = u16::try_from(bits.ReadBitsNoRefillZero(v) + (1 << v)).unwrap();
            let v = symlen[idx] as i32;
            idx += 1;
            if v >= 8 {
                return None;
            }
            let space = u16::try_from(bits.ReadBitsNoRefill(v + 1) + (1 << (v + 1)) - 1).unwrap();
            ranges.push(HuffRange { symbol, num });
            syms_used += num;
            symbol += num + space;
        }

        if symbol >= 256 || syms_used >= num_symbols || symbol + num_symbols - syms_used > 256 {
            return None;
        }

        ranges.push(HuffRange {
            symbol,
            num: num_symbols - syms_used,
        });

        Some(ranges)
    }

    fn Krak_DecodeRecursive(
        &mut self,
        src_org: Pointer,
        src_size: usize,
        mut output: Pointer,
        output_size: usize,
        scratch: Pointer,
    ) -> Option<usize> {
        let mut src = src_org;
        let output_end = output + output_size;
        let src_end = src + src_size;

        if src_size < 6 {
            return None;
        }

        let n = self.get_as_usize(src) & 0x7f;
        if n < 2 {
            return None;
        }

        if (self.get_byte(src) & 0x80) != 0 {
            src += 1;
            for _ in 0..n {
                let mut decoded_size = 0;
                let output_size = output_end - output;
                let dec = self.Kraken_DecodeBytes(
                    &mut output,
                    src,
                    src_end,
                    &mut decoded_size,
                    output_size,
                    true,
                    scratch,
                );
                output += decoded_size;
                src += dec;
            }
            if output != output_end {
                return None;
            }
            Some(src - src_org)
        } else {
            let mut decoded_size = 0;
            let dec = self.Kraken_DecodeMultiArray(
                src,
                src_end,
                output,
                output_end,
                &mut Vec::new(),
                &mut Vec::new(),
                1,
                &mut decoded_size,
                true,
                scratch,
            );
            output += decoded_size;
            if output != output_end {
                return None;
            }
            Some(dec)
        }
    }

    pub fn Kraken_DecodeMultiArray(
        &mut self,
        src_org: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_end: Pointer,
        array_data: &mut Vec<Pointer>,
        array_lens: &mut Vec<usize>,
        array_count: usize,
        total_size_out: &mut usize,
        force_memmove: bool,
        scratch: Pointer,
    ) -> usize {
        let mut src = src_org;

        assert!(src_end - src >= 4, "{}", src_end - src);

        let mut decoded_size = 0;
        let mut num_arrays_in_file = self.get_as_usize(src);
        src += 1;
        assert_ne!(num_arrays_in_file & 0x80, 0);
        num_arrays_in_file &= 0x3f;

        let mut total_size = 0;

        if num_arrays_in_file == 0 {
            for _ in 0..array_count {
                let mut chunk_dst = dst;
                let dec = self.Kraken_DecodeBytes(
                    &mut chunk_dst,
                    src,
                    src_end,
                    &mut decoded_size,
                    dst_end - dst,
                    force_memmove,
                    scratch,
                );
                dst += decoded_size;
                array_data.push(dst);
                array_lens.push(decoded_size);
                src += dec;
                total_size += decoded_size;
            }
            *total_size_out = total_size;
            return src - src_org; // not supported yet
        }

        let mut entropy_array_data = [Default::default(); 63];
        let mut entropy_array_size = [0; 63];

        // First loop just decodes everything to scratch
        let mut scratch_cur = scratch;

        for i in 0..num_arrays_in_file {
            let mut chunk_dst = scratch_cur;
            let dec = self.Kraken_DecodeBytes(
                &mut chunk_dst,
                src,
                src_end,
                &mut decoded_size,
                usize::MAX,
                force_memmove,
                scratch_cur,
            );
            entropy_array_data[i] = chunk_dst;
            entropy_array_size[i] = decoded_size;
            scratch_cur += decoded_size;
            total_size += decoded_size;
            src += dec;
        }
        *total_size_out = total_size;

        assert!(src_end - src >= 3, "{}", src_end - src);

        let q = self.get_bytes_as_usize_le(src, 2);
        src += 2;

        let num_indexes = self.Kraken_GetBlockSize(src, src_end, total_size).unwrap();

        let mut num_lens = num_indexes - array_count;
        assert_ne!(num_lens, 0);

        let mut interval_lenlog2 = scratch_cur;
        scratch_cur += num_indexes;

        let mut interval_indexes = scratch_cur;
        scratch_cur += num_indexes;

        if (q & 0x8000) != 0 {
            let mut size_out = 0;
            let n = self.Kraken_DecodeBytes(
                &mut interval_indexes,
                src,
                src_end,
                &mut size_out,
                num_indexes,
                true,
                scratch_cur,
            );
            assert_eq!(size_out, num_indexes);
            src += n;

            for i in 0..num_indexes {
                let t = self.get_byte(interval_indexes + i);
                self.set(interval_lenlog2 + i, t >> 4);
                self.set(interval_indexes + i, t & 0xF);
            }

            num_lens = num_indexes;
        } else {
            let lenlog2_chunksize = num_indexes - array_count;

            let mut size_out = 0;
            let n = self.Kraken_DecodeBytes(
                &mut interval_indexes,
                src,
                src_end,
                &mut size_out,
                num_indexes,
                false,
                scratch_cur,
            );
            src += n;

            let n = self.Kraken_DecodeBytes(
                &mut interval_lenlog2,
                src,
                src_end,
                &mut size_out,
                lenlog2_chunksize,
                false,
                scratch_cur,
            );
            src += n;

            for i in 0..lenlog2_chunksize {
                assert!(
                    self.get_byte(interval_lenlog2 + i) <= 16,
                    "{} {}",
                    i,
                    self.get_byte(interval_lenlog2 + i)
                );
            }
        }

        let mut decoded_intervals = Vec::with_capacity(num_lens);

        let varbits_complen = q & 0x3FFF;
        assert!(
            src_end - src >= varbits_complen,
            "{} {}",
            src_end - src,
            varbits_complen
        );

        let mut f = src;
        let mut bits_f = 0;
        let mut bitpos_f = 24;

        let src_end_actual = src + varbits_complen;

        let mut b = src_end_actual;
        let mut bits_b = 0;
        let mut bitpos_b = 24;

        const BITMASKS: [usize; 32] = [
            0x1, 0x3, 0x7, 0xf, 0x1f, 0x3f, 0x7f, 0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff,
            0x7fff, 0xffff, 0x1ffff, 0x3ffff, 0x7ffff, 0xfffff, 0x1fffff, 0x3fffff, 0x7fffff,
            0xffffff, 0x1ffffff, 0x3ffffff, 0x7ffffff, 0xfffffff, 0x1fffffff, 0x3fffffff,
            0x7fffffff, 0xffffffff,
        ];

        for i in (0..num_lens).step_by(2) {
            bits_f |= self.get_bytes_as_usize_be(f, 4) >> (24 - bitpos_f);
            f += (bitpos_f as usize + 7) >> 3;

            bits_b |= self.get_bytes_as_usize_le(b - 1, 4) >> (24 - bitpos_b);
            b -= (bitpos_b as usize + 7) >> 3;

            let numbits_f = self.get_byte(interval_lenlog2 + i + 0);
            let numbits_b = self.get_byte(interval_lenlog2 + i + 1);

            bits_f = (bits_f | 1).rotate_left(numbits_f as _);
            bitpos_f += numbits_f - 8 * ((bitpos_f + 7) >> 3);

            bits_b = (bits_b | 1).rotate_left(numbits_b as _);
            bitpos_b += numbits_b - 8 * ((bitpos_b + 7) >> 3);

            let value_f = bits_f & BITMASKS[numbits_f as usize];
            bits_f &= !BITMASKS[numbits_f as usize];

            let value_b = bits_b & BITMASKS[numbits_b as usize];
            bits_b &= !BITMASKS[numbits_b as usize];

            decoded_intervals.push(value_f);
            decoded_intervals.push(value_b);
        }

        // read final one since above loop reads 2
        if (num_lens & 1) == 1 {
            bits_f |= self.get_bytes_as_usize_be(f, 4) >> (24 - bitpos_f);
            let numbits_f = self.get_byte(interval_lenlog2 + num_lens - 1);
            bits_f = (bits_f | 1).rotate_left(numbits_f as _);
            let value_f = bits_f & BITMASKS[numbits_f as usize];
            decoded_intervals.push(value_f);
        }

        assert!(!self.get_as_bool(interval_indexes + num_indexes - 1));

        let mut indi = 0;
        let mut leni = 0;
        let increment_leni = (q & 0x8000) != 0;

        for arri in 0..array_count {
            array_data.push(dst);
            assert!(indi < num_indexes, "{} {}", indi, num_indexes);

            loop {
                let source = self.get_as_usize(interval_indexes + indi);
                if source == 0 {
                    break;
                }
                indi += 1;
                assert!(
                    source <= num_arrays_in_file,
                    "{} {}",
                    source,
                    num_arrays_in_file
                );
                assert!(leni < num_lens, "{} {}", leni, num_lens);
                let cur_len = decoded_intervals[leni];
                leni += 1;
                let bytes_left = entropy_array_size[source - 1];
                assert!(cur_len <= bytes_left, "{} {}", cur_len, bytes_left);
                assert!(cur_len <= dst_end - dst, "{} {}", cur_len, dst_end - dst);
                let blksrc = entropy_array_data[source - 1];
                entropy_array_size[source - 1] -= cur_len;
                entropy_array_data[source - 1] += cur_len;
                let dstx = dst;
                dst += cur_len;
                self.memcpy(dstx, blksrc, cur_len);
            }
            if increment_leni {
                leni += 1;
            }
            array_lens.push(dst - array_data[arri]);
        }

        assert_eq!(indi, num_indexes);
        assert_eq!(leni, num_lens);

        for &i in entropy_array_size[..num_arrays_in_file].iter() {
            assert_eq!(i, 0);
        }

        src_end_actual - src_org
    }

    fn Kraken_GetBlockSize(
        &mut self,
        src_org: Pointer,
        src_end: Pointer,
        dest_capacity: usize,
    ) -> Option<usize> {
        let mut src = src_org;
        let src_size;
        let dst_size;

        if src_end - src < 2 {
            return None;
        } // too few bytes

        let chunk_type = (self.get_byte(src) >> 4) & 0x7;
        if chunk_type == 0 {
            if self.get_byte(src) >= 0x80 {
                // In this mode, memcopy stores the length in the bottom 12 bits.
                src_size = self.get_bytes_as_usize_be(src, 2) & 0xFFF;
                src += 2;
            } else {
                if src_end - src < 3 {
                    return None;
                } // too few bytes
                src_size = self.get_bytes_as_usize_be(src, 3);
                if (src_size & !0x3ffff) != 0 {
                    return None;
                } // reserved bits must not be set
                src += 3;
            }
            if src_size > dest_capacity || src_end - src < src_size {
                return None;
            }
            return Some(src_size);
        }

        if chunk_type >= 6 {
            return None;
        }

        // In all the other modes, the initial bytes encode
        // the src_size and the dst_size
        if self.get_byte(src) >= 0x80 {
            if src_end - src < 3 {
                return None;
            } // too few bytes

            // short mode, 10 bit sizes
            let bits = self.get_bytes_as_usize_be(src, 3);
            src_size = bits & 0x3ff;
            dst_size = src_size + ((bits >> 10) & 0x3ff) + 1;
            src += 3;
        } else {
            // long mode, 18 bit sizes
            if src_end - src < 5 {
                return None;
            } // too few bytes
            let bits = self.get_bytes_as_usize_be(src, 5);
            src_size = bits & 0x3ffff;
            dst_size = (((bits >> 18) | (self.get_as_usize(src) << 14)) & 0x3FFFF) + 1;
            if src_size >= dst_size {
                return None;
            }
            src += 5;
        }
        if src_end - src < src_size || dst_size > dest_capacity {
            return None;
        }
        Some(dst_size)
    }

    fn Krak_DecodeRLE(
        &mut self,
        src: Pointer,
        src_size: usize,
        mut dst: Pointer,
        dst_size: usize,
        scratch: Pointer,
    ) -> Option<usize> {
        if src_size <= 1 {
            if src_size != 1 {
                return None;
            }
            self.memset(dst, self.get_byte(src), dst_size);
            return Some(1);
        }
        let dst_end = dst + dst_size;
        let mut cmd_ptr = src + 1;
        let mut cmd_ptr_end = src + src_size;
        // Unpack the first X bytes of the command buffer?
        if self.get_as_bool(src) {
            let mut dst_ptr = scratch;
            let mut dec_size = 0;
            let n = self.Kraken_DecodeBytes(
                &mut dst_ptr,
                src,
                src + src_size,
                &mut dec_size,
                usize::MAX,
                true,
                scratch,
            );
            assert!(n > 0, "{}", n);
            let cmd_len = src_size - n + dec_size;
            self.memcpy(dst_ptr + dec_size, src + n, src_size - n);
            cmd_ptr = dst_ptr;
            cmd_ptr_end = dst_ptr + cmd_len;
        }

        let mut rle_byte = 0;

        while cmd_ptr < cmd_ptr_end {
            let cmd = self.get_as_usize(cmd_ptr_end - 1);
            if cmd > 0x2f {
                cmd_ptr_end -= 1;
                let bytes_to_copy = !cmd & 0xF;
                let bytes_to_rle = cmd >> 4;
                if dst_end - dst < bytes_to_copy + bytes_to_rle
                    || cmd_ptr_end - cmd_ptr < bytes_to_copy
                {
                    return None;
                }
                self.memcpy(dst, cmd_ptr, bytes_to_copy);
                cmd_ptr += bytes_to_copy;
                dst += bytes_to_copy;
                self.memset(dst, rle_byte, bytes_to_rle);
                dst += bytes_to_rle;
            } else if cmd >= 0x10 {
                cmd_ptr_end -= 2;
                let data = self.get_bytes_as_usize_le(cmd_ptr_end, 2) - 4096;
                let bytes_to_copy = data & 0x3F;
                let bytes_to_rle = data >> 6;
                if dst_end - dst < bytes_to_copy + bytes_to_rle
                    || cmd_ptr_end - cmd_ptr < bytes_to_copy
                {
                    return None;
                }
                self.memcpy(dst, cmd_ptr, bytes_to_copy);
                cmd_ptr += bytes_to_copy;
                dst += bytes_to_copy;
                self.memset(dst, rle_byte, bytes_to_rle);
                dst += bytes_to_rle;
            } else if cmd == 1 {
                rle_byte = self.get_byte(cmd_ptr);
                cmd_ptr += 1;
                cmd_ptr_end -= 1;
            } else if cmd >= 9 {
                cmd_ptr_end -= 2;
                let bytes_to_rle = (self.get_bytes_as_usize_le(cmd_ptr_end, 2) - 0x8ff) * 128;
                if dst_end - dst < bytes_to_rle {
                    return None;
                }
                self.memset(dst, rle_byte, bytes_to_rle);
                dst += bytes_to_rle;
            } else {
                cmd_ptr_end -= 2;
                let bytes_to_copy = (self.get_bytes_as_usize_le(cmd_ptr_end, 2) - 511) * 64;
                if cmd_ptr_end - cmd_ptr < bytes_to_copy || dst_end - dst < bytes_to_copy {
                    return None;
                }
                self.memcpy(dst, cmd_ptr, bytes_to_copy);
                dst += bytes_to_copy;
                cmd_ptr += bytes_to_copy;
            }
        }
        if cmd_ptr_end != cmd_ptr {
            return None;
        }

        if dst != dst_end {
            return None;
        }

        Some(src_size)
    }

    fn Krak_DecodeTans(
        &mut self,
        mut src: Pointer,
        src_size: usize,
        dst: Pointer,
        dst_size: usize,
    ) -> Option<usize> {
        if src_size < 8 || dst_size < 5 {
            return None;
        }

        let mut src_end = src + src_size;

        let mut br = BitReader {
            bitpos: 24,
            bits: 0,
            p: src,
            p_end: src_end,
        };
        br.Refill(self);

        // reserved bit
        if br.ReadBitNoRefill() {
            return None;
        }

        let l_bits = br.ReadBitsNoRefill(2) + 8;

        let tans_data = TansDecoder::decode_table(self, &mut br, l_bits)?;

        src = br.p - (24 - br.bitpos) / 8;

        if src >= src_end {
            return None;
        }

        let mut decoder = TansDecoder::default();
        decoder.dst = dst;
        decoder.dst_end = dst + dst_size - 5;

        decoder.lut = decoder.init_lut(&tans_data, l_bits as u32);

        // Read out the initial state
        let l_mask = (1 << l_bits) - 1;
        let mut bits_f = self.get_bytes_as_usize_le(src, 4);
        src += 4;
        src_end -= 4;
        let mut bits_b = self.get_bytes_as_usize_be(src_end, 4);
        let mut bitpos_f = 32;
        let mut bitpos_b = 32;

        // Read first two.
        decoder.state[0] = bits_f & l_mask;
        decoder.state[1] = bits_b & l_mask;
        bits_f >>= l_bits;
        bitpos_f -= l_bits;
        bits_b >>= l_bits;
        bitpos_b -= l_bits;

        // Read next two.
        decoder.state[2] = bits_f & l_mask;
        decoder.state[3] = bits_b & l_mask;
        bits_f >>= l_bits;
        bitpos_f -= l_bits;
        bits_b >>= l_bits;
        bitpos_b -= l_bits;

        // Refill more bits
        bits_f |= self.get_bytes_as_usize_le(src, 4) << bitpos_f;
        src += (31 - bitpos_f) >> 3;
        bitpos_f |= 24;

        // Read final state variable
        decoder.state[4] = bits_f & l_mask;
        bits_f >>= l_bits;
        bitpos_f -= l_bits;

        decoder.bits_f = bits_f;
        decoder.ptr_f = src - (bitpos_f >> 3);
        decoder.bitpos_f = (bitpos_f & 7) as _;

        decoder.bits_b = bits_b;
        decoder.ptr_b = src_end + (bitpos_b >> 3);
        decoder.bitpos_b = (bitpos_b & 7) as _;

        decoder.decode(self);

        Some(src_size)
    }
}
