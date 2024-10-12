use crate::algorithm::Algorithm;
use crate::bit_reader::{BitReader, BitReader2};
use crate::error::End::Idx;
use crate::error::{ErrorContext, Res, ResultBuilder, WithContext};
use crate::huffman::{HuffRange, HuffReader, BASE_PREFIX};
use crate::pointer::{IntPointer, Pointer, PointerDest};
use crate::tans::TansDecoder;
use std::fmt::Debug;

pub(crate) struct Core<'a> {
    pub input: &'a [u8],
    pub output: &'a mut [u8],
    pub scratch: Vec<u8>,
    pub tmp: Vec<u8>,
    pub src: Pointer,
    pub dst: Pointer,
    pub dst_end: Pointer,
}

impl Core<'_> {
    pub fn new<'a>(
        input: &'a [u8],
        output: &'a mut [u8],
        offset: usize,
        out_len: usize,
    ) -> Core<'a> {
        Core {
            input,
            output,
            scratch: Vec::new(),
            tmp: Vec::new(),
            src: Pointer::input(0),
            dst: Pointer::output(offset),
            dst_end: Pointer::output(offset + out_len),
        }
    }

    /// Decode one 256kb big quantum block. It's divided into two 128k blocks
    /// internally that are compressed separately but with a shared history.
    pub fn decode_quantum<T: Algorithm + Debug>(&mut self, algorithm: T) -> Res<usize> {
        let mut written_bytes = 0;
        let src_end = Pointer::input(self.input.len());
        let dst_start = Pointer::output(0);
        let mut src_used;

        while self.dst_end > self.dst {
            let dst_count = std::cmp::min((self.dst_end - self.dst)?, 0x20000);
            self.assert_le(4, (src_end - self.src)?)?;
            let chunkhdr = self.get_be_bytes(self.src, 3).at(self)?;
            log::debug!("index: {}, chunk header: {}", self.src.index, chunkhdr);
            if (chunkhdr & 0x800000) == 0 {
                log::debug!("Stored as entropy without any match copying.");
                let mut out = self.dst;
                src_used = self
                    .decode_bytes(
                        &mut out,
                        self.src,
                        src_end,
                        &mut written_bytes,
                        dst_count,
                        false,
                        Pointer::scratch(0),
                    )
                    .at(self)?;
                self.assert_eq(written_bytes, dst_count)?;
            } else {
                self.src += 3;
                src_used = chunkhdr & 0x7FFFF;
                let mode = (chunkhdr >> 19) & 0xF;
                self.assert_le(src_used, (src_end - self.src)?)?;
                if src_used < dst_count {
                    log::debug!("processing with {:?}", algorithm);
                    algorithm
                        .process(
                            self, mode, self.src, src_used, dst_start, self.dst, dst_count,
                        )
                        .at(self)?;
                } else if src_used > dst_count || mode != 0 {
                    self.raise(format!(
                        "Bad data. src_used: {}, dst_count: {}, mode: {}",
                        src_used, dst_count, mode
                    ))?;
                } else {
                    log::debug!("copying {} bytes", dst_count);
                    self.copy_bytes(self.dst, self.src, dst_count).at(self)?;
                }
            }
            self.src += src_used;
            self.dst += dst_count;
        }

        Ok(self.src.index)
    }

    /// Unpacks the packed 8 bit offset and lengths into 32 bit.
    pub fn unpack_offsets(
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
    ) -> Res<()> {
        let mut n;
        let mut u32_len_stream_size = 0usize;
        let offs_stream_org = offs_stream;

        let mut bits_a = BitReader {
            bitpos: 24,
            bits: 0,
            p: src,
            p_end: src_end,
        };
        bits_a.refill(self).at(self)?;

        let mut bits_b = BitReader {
            bitpos: 24,
            bits: 0,
            p: src_end,
            p_end: src,
        };
        bits_b.refill_backwards(self).at(self)?;

        if !excess_flag {
            self.assert_le(0x2000, bits_b.bits)?;
            n = bits_b.leading_zeros();
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.refill_backwards(self).at(self)?;
            n += 1;
            u32_len_stream_size = ((bits_b.bits >> (32 - n)) - 1) as usize;
            bits_b.bitpos += n;
            bits_b.bits <<= n;
            bits_b.refill_backwards(self).at(self)?;
        }

        if multi_dist_scale == 0 {
            // Traditional way of coding offsets
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            while packed_offs_stream != packed_offs_stream_end {
                let d_a = bits_a
                    .read_distance(self, self.get_byte(packed_offs_stream)?.into())
                    .at(self)?;
                self.set_int(offs_stream, -d_a).at(self)?;
                offs_stream += 1;
                packed_offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                let d_b = bits_b
                    .read_distance_b(self, self.get_byte(packed_offs_stream)?.into())
                    .at(self)?;
                self.set_int(offs_stream, -d_b).at(self)?;
                offs_stream += 1;
                packed_offs_stream += 1;
            }
        } else {
            // New way of coding offsets
            let packed_offs_stream_end = packed_offs_stream + packed_offs_stream_size;
            let mut cmd;
            let mut offs;
            while packed_offs_stream != packed_offs_stream_end {
                cmd = self.get_byte(packed_offs_stream)? as i32;
                packed_offs_stream += 1;
                self.assert_le(cmd >> 3, 26)?;
                offs = ((8 + (cmd & 7)) << (cmd >> 3))
                    | bits_a.read_more_than24bits(self, cmd >> 3).at(self)?;
                self.set_int(offs_stream, 8 - offs).at(self)?;
                offs_stream += 1;
                if packed_offs_stream == packed_offs_stream_end {
                    break;
                }
                cmd = i32::from(self.get_byte(packed_offs_stream)?);
                packed_offs_stream += 1;
                self.assert_le(cmd >> 3, 26)?;
                offs = ((8 + (cmd & 7)) << (cmd >> 3))
                    | bits_b.read_more_than_24_bits_b(self, cmd >> 3).at(self)?;
                self.set_int(offs_stream, 8 - offs).at(self)?;
                offs_stream += 1;
            }
            if multi_dist_scale != 1 {
                self.combine_scaled_offset_arrays(
                    &offs_stream_org,
                    (offs_stream - offs_stream_org)?,
                    multi_dist_scale,
                    &packed_offs_stream_extra,
                )
                .at(self)?;
            }
        }
        let mut u32_len_stream_buf = [0u32; 512]; // max count is 128kb / 256 = 512
        self.assert_le(u32_len_stream_size, 512)?;

        let mut u32_len_stream = 0;
        for (i, dst) in self
            .slice_mut(&mut u32_len_stream_buf, 0, Idx(u32_len_stream_size))?
            .iter_mut()
            .enumerate()
        {
            if i % 2 == 0 {
                *dst = bits_a.read_length(self).at(self)? as u32
            } else {
                *dst = bits_b.read_length_b(self).at(self)? as u32
            }
        }

        bits_a.p -= (24 - bits_a.bitpos) >> 3;
        bits_b.p += (24 - bits_b.bitpos) >> 3;

        self.assert_eq(bits_a.p, bits_b.p)?;

        for i in 0..packed_litlen_stream_size {
            let mut v = u32::from(self.get_byte(packed_litlen_stream + i)?);
            if v == 255 {
                v = u32_len_stream_buf[u32_len_stream] + 255;
                u32_len_stream += 1;
            }
            self.set_int(len_stream + i, (v + 3) as i32).at(self)?;
        }
        self.assert_eq(u32_len_stream, u32_len_stream_size)?;
        Ok(())
    }

    fn combine_scaled_offset_arrays(
        &mut self,
        offs_stream: &IntPointer,
        offs_stream_size: usize,
        scale: i32,
        low_bits: &Pointer,
    ) -> Res<()> {
        for i in 0..offs_stream_size {
            let low = self.get_byte(low_bits + i)? as i32;
            let scaled = scale * self.get_int(offs_stream + i).at(self)? - low;
            self.set_int(offs_stream + i, scaled).at(self)?
        }
        Ok(())
    }

    pub fn decode_bytes(
        &mut self,
        output: &mut Pointer,
        mut src: Pointer,
        src_end: Pointer,
        decoded_size: &mut usize,
        output_size: usize,
        force_memmove: bool,
        mut scratch: Pointer,
    ) -> Res<usize> {
        let src_org = src;
        let src_size;
        let dst_size;

        self.assert_le(2, (src_end - src)?)?;

        let chunk_type = (self.get_byte(src + 0)? as usize >> 4) & 0x7;
        if chunk_type == 0 {
            if (self.get_byte(src + 0)? as usize) >= 0x80 {
                // In this mode, memcopy stores the length in the bottom 12 bits.
                src_size = (((self.get_byte(src + 0)? as usize) << 8)
                    | (self.get_byte(src + 1)? as usize))
                    & 0xFFF;
                src += 2;
            } else {
                self.assert_le(3, (src_end - src)?)?;
                src_size = self.get_be_bytes(src, 3).at(self)?;
                // reserved bits must not be set
                self.assert_eq(src_size & !0x3ffff, 0)?;
                src += 3;
            }
            self.assert_le(src_size, output_size)?;
            self.assert_le(src_size, (src_end - src)?)?;
            *decoded_size = src_size;
            if force_memmove {
                self.copy_bytes(*output, src, src_size).at(self)?;
            } else {
                *output = src;
            }
            return Ok((src + src_size - src_org)?);
        }

        // In all the other modes, the initial bytes encode
        // the src_size and the dst_size
        if self.get_byte(src)? >= 0x80 {
            self.assert_le(3, (src_end - src)?)?;

            // short mode, 10 bit sizes
            let bits = self.get_be_bytes(src, 3).at(self)?;
            src_size = bits & 0x3ff;
            dst_size = src_size + ((bits >> 10) & 0x3ff) + 1;
            src += 3;
        } else {
            // long mode, 18 bit sizes
            self.assert_le(5, (src_end - src)?)?;
            let bits = self.get_be_bytes(src + 1, 4).at(self)?;
            src_size = bits & 0x3ffff;
            dst_size = (((bits >> 18) | ((self.get_byte(src + 0)? as usize) << 14)) & 0x3FFFF) + 1;
            self.assert_lt(src_size, dst_size)?;
            src += 5;
        }
        self.assert_le(src_size, (src_end - src)?)?;
        self.assert_le(dst_size, output_size)?;

        let dst = *output;
        if dst.into == PointerDest::Scratch {
            scratch += dst_size;
        }

        let src_used = match chunk_type {
            2 | 4 => self.decode_bytes_type12(src, src_size, dst, dst_size, chunk_type >> 1),
            5 => self.decode_recursive(src, src_size, dst, dst_size, scratch),
            3 => self.decode_rle(src, src_size, dst, dst_size, scratch),
            1 => self.decode_tans(src, src_size, dst, dst_size),
            _ => self.raise(format!("{}", chunk_type))?,
        }
        .at(self)?;
        self.assert_eq(src_used, src_size)
            .message(|msg| format!("{} for chunk type {}", msg.unwrap_or(""), chunk_type))?;
        *decoded_size = dst_size;
        Ok((src + src_size - src_org)?)
    }

    fn decode_bytes_type12(
        &mut self,
        mut src: Pointer,
        src_size: usize,
        output: Pointer,
        output_size: usize,
        chunk_type: usize,
    ) -> Res<usize> {
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
        bits.refill(self).at(self)?;

        let mut code_prefix = BASE_PREFIX;
        let mut syms = [0; 1280];
        let num_syms;
        if !bits.read_bit_no_refill() {
            num_syms = self
                .huff_read_code_lengths_old(&mut bits, &mut syms, &mut code_prefix)
                .at(self)?;
        } else if !bits.read_bit_no_refill() {
            num_syms = self
                .huff_read_code_lengths_new(&mut bits, &mut syms, &mut code_prefix)
                .at(self)?;
        } else {
            self.raise("Bad data".into())?;
            unreachable!()
        }
        src = (bits.p - ((24 - bits.bitpos) / 8))?;

        if num_syms == 1 {
            // no test coverage
            self.memset(output, syms[0], output_size).at(self)?;
            return Ok((src - src_end)?);
        }

        let rev_lut = self.make_lut(&code_prefix, &syms).at(self)?;

        if chunk_type == 1 {
            self.assert_le(3, (src_end - src)?)?;
            split_mid = self.get_le_bytes(src, 2).at(self)?;
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
            hr.decode_bytes(self, &rev_lut).at(self)?;
        } else {
            self.assert_le(6, (src_end - src)?)?;

            half_output_size = (output_size + 1) >> 1;
            split_mid = self.get_le_bytes(src, 3).at(self)?;
            src += 3;
            self.assert_le(split_mid, (src_end - src)?)?;
            src_mid = src + split_mid;
            split_left = self.get_le_bytes(src, 2).at(self)?;
            src += 2;
            self.assert_le(split_left + 2, (src_end - src)?)?;
            self.assert_le(3, (src_end - src_mid)?)?;
            split_right = self.get_le_bytes(src_mid, 2).at(self)?;
            self.assert_le(split_right + 2, (src_end - (src_mid + 2))?)?;

            let mut hr = HuffReader {
                output,
                output_end: output + half_output_size,
                src,
                src_end: src_mid,
                src_mid_org: src + split_left,
                src_mid: src + split_left,
                ..Default::default()
            };
            hr.decode_bytes(self, &rev_lut).at(self)?;

            let mut hr = HuffReader {
                output: output + half_output_size,
                output_end: output + output_size,
                src: src_mid + 2,
                src_end,
                src_mid_org: src_mid + 2 + split_right,
                src_mid: src_mid + 2 + split_right,
                ..Default::default()
            };
            hr.decode_bytes(self, &rev_lut).at(self)?;
        }
        Ok(src_size)
    }

    fn huff_read_code_lengths_old(
        &mut self,
        bits: &mut BitReader,
        syms: &mut [u8; 1280],
        code_prefix: &mut [usize; 12],
    ) -> Res<i32> {
        if bits.read_bit_no_refill() {
            let mut sym = 0;
            let mut num_symbols = 0;
            let mut avg_bits_x4 = 32;
            let forced_bits = bits.read_bits_no_refill(2);

            let thres_for_valid_gamma_bits = 1 << (31 - (20 >> forced_bits));
            let mut skip_initial_zeros = bits.read_bit(self).at(self)?;
            while sym != 256 {
                if skip_initial_zeros {
                    skip_initial_zeros = false;
                } else {
                    // Run of zeros
                    self.assert_ne(bits.bits & 0xff000000, 0)?;
                    sym += bits.read_bits_no_refill(2 * (bits.leading_zeros() + 1)) - 2 + 1;
                    if sym >= 256 {
                        break;
                    }
                }
                bits.refill(self).at(self)?;
                // Read out the gamma value for the # of symbols
                self.assert_ne(bits.bits & 0xff000000, 0)?;
                let mut n = bits.read_bits_no_refill(2 * (bits.leading_zeros() + 1)) - 2 + 1;
                // Overflow
                self.assert_le(sym + n, 256)?;
                bits.refill(self).at(self)?;
                num_symbols += n;
                loop {
                    // too big gamma value?
                    self.assert_le(thres_for_valid_gamma_bits, bits.bits)?;

                    let lz = bits.leading_zeros();
                    let v =
                        bits.read_bits_no_refill(lz + forced_bits + 1) + ((lz - 1) << forced_bits);
                    let codelen = (-(v & 1) ^ (v >> 1)) + ((avg_bits_x4 + 2) >> 2);
                    self.assert_le(1, codelen)?;
                    self.assert_le(codelen, 11)?;
                    avg_bits_x4 = codelen + ((3 * avg_bits_x4 + 2) >> 2);
                    bits.refill(self).at(self)?;
                    syms[code_prefix[usize::try_from(codelen).unwrap()]] = sym as _;
                    code_prefix[usize::try_from(codelen).unwrap()] += 1;
                    sym += 1;
                    n -= 1;
                    if n == 0 {
                        break;
                    }
                }
            }
            self.assert_eq(sym, 256)?;
            self.assert_le(2, num_symbols)?;
            Ok(num_symbols)
        } else {
            // Sparse symbol encoding
            let num_symbols = bits.read_bits_no_refill(8);
            self.assert_ne(num_symbols, 0)?;
            if num_symbols == 1 {
                syms[0] = bits.read_bits_no_refill(8) as _;
            } else {
                let codelen_bits = bits.read_bits_no_refill(3);
                self.assert_le(codelen_bits, 4)?;
                for _ in 0..num_symbols {
                    bits.refill(self).at(self)?;
                    let sym = bits.read_bits_no_refill(8) as u8;
                    let codelen = bits.read_bits_no_refill_zero(codelen_bits) + 1;
                    assert!(codelen <= 11, "{}", codelen);
                    syms[code_prefix[usize::try_from(codelen).unwrap()]] = sym;
                    code_prefix[usize::try_from(codelen).unwrap()] += 1;
                }
            }
            Ok(num_symbols)
        }
    }

    fn huff_read_code_lengths_new(
        &mut self,
        bits: &mut BitReader,
        syms: &mut [u8; 1280],
        code_prefix: &mut [usize; 12],
    ) -> Res<i32> {
        let forced_bits = bits.read_bits_no_refill(2);

        let num_symbols = bits.read_bits_no_refill(8) + 1;

        let fluff = bits.read_fluff(num_symbols);

        let mut code_len = [0; 512];
        let mut br2 = BitReader2 {
            bitpos: ((bits.bitpos - 24) & 7) as u32,
            p_end: bits.p_end,
            p: (bits.p - ((24 - bits.bitpos + 7) >> 3) as u32)?,
        };

        self.decode_golomb_rice_lengths(&mut code_len[..num_symbols as usize + fluff], &mut br2)
            .at(self)?;
        self.decode_golomb_rice_bits(
            &mut code_len[..num_symbols as usize],
            forced_bits as usize,
            &mut br2,
        )
        .at(self)?;

        // Reset the bits decoder.
        bits.bitpos = 24;
        bits.p = br2.p;
        bits.bits = 0;
        bits.refill(self).at(self)?;
        bits.bits <<= br2.bitpos;
        bits.bitpos += br2.bitpos as i32;

        let mut running_sum = 0x1e;
        for len in code_len[..num_symbols as usize].iter_mut() {
            let mut v = *len as i32;
            v = (!(v & 1) + 1) ^ (v >> 1);
            *len = (v + (running_sum >> 2) + 1) as u8;
            self.assert_le(1, *len)?;
            self.assert_le(*len, 11)?;
            running_sum += v;
        }

        let ranges = self
            .convert_to_ranges(num_symbols, fluff, &code_len, bits)
            .at(self)?;

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

        Ok(num_symbols)
    }

    pub fn decode_golomb_rice_lengths(
        &mut self,
        mut dst: &mut [u8],
        br: &mut BitReader2,
    ) -> Res<()> {
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
        self.assert_lt(p, p_end)?;

        let mut count = -(br.bitpos as i32);
        let mut v = (self.get_byte(p)? as usize) & (255 >> br.bitpos);
        p += 1;
        loop {
            if v == 0 {
                count += 8;
            } else {
                let x = K_RICE_CODE_BITS2VALUE[v] as i32;
                let len = dst.len().min(4);
                dst[..len].copy_from_slice(&(count + (x & 0x0f0f0f0f)).to_le_bytes()[..len]);
                if dst.len() > 4 {
                    let dst = &mut dst[4..];
                    let len = dst.len().min(4);
                    dst[..len].copy_from_slice(&((x >> 4) & 0x0f0f0f0f).to_le_bytes()[..len]);
                }
                let step = K_RICE_CODE_BITS2LEN[v] as usize;
                if dst.len() <= step {
                    // went too far, step back
                    for _ in dst.len()..step {
                        v &= v - 1;
                    }
                    break;
                }
                dst = &mut dst[step..];
                count = x >> 28;
            }
            self.assert_lt(p, p_end)?;
            v = self.get_byte(p)? as _;
            p += 1;
        }
        // step back if byte not finished
        let mut bitpos = 0;
        if (v & 1) == 0 {
            self.assert_ne(v, 0)?;
            bitpos = 8 - v.trailing_zeros();
            p -= 1;
        }
        br.p = p;
        br.bitpos = bitpos;
        Ok(())
    }

    fn decode_golomb_rice_bits(
        &mut self,
        mut dst: &mut [u8],
        bitcount: usize,
        br: &mut BitReader2,
    ) -> Res<()> {
        if bitcount == 0 {
            return Ok(());
        }
        let mut p = br.p;
        let bitpos = br.bitpos;

        let bits_required = bitpos as usize + bitcount * dst.len();
        let bytes_required = (bits_required + 7) >> 3;
        self.assert_lt(bytes_required, (br.p_end - p)?)?;

        br.p = p + (bits_required >> 3);
        br.bitpos = (bits_required & 7) as u32;

        while !dst.is_empty() {
            let bits = match bitcount {
                1 => {
                    // Read the next byte
                    let mut bits =
                        ((self.get_be_bytes(p, 4).at(self)? >> (24 - bitpos)) & 0xFF) as u64;
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
                        ((self.get_be_bytes(p, 4).at(self)? >> (16 - bitpos)) & 0xFFFF) as u64;
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
                        ((self.get_be_bytes(p, 4).at(self)? >> (8 - bitpos)) & 0xffffff) as u64;
                    p += 3;
                    // Expand each bit into each byte of the uint64.
                    bits = (bits | (bits << 20)) & 0xFFF00000FFF;
                    bits = (bits | (bits << 10)) & 0x3F003F003F003F;
                    bits = (bits | (bits << 5)) & 0x0707070707070707;
                    bits
                }
                _ => self.raise(format!("Unexpected bitcount {}", bitcount))?,
            };
            let mut bytes = [0; 8];
            let len = dst.len().min(8);
            bytes[..len].copy_from_slice(&dst[..len]);
            let v = (u64::from_le_bytes(bytes) << bitcount) + bits.swap_bytes();
            dst[..len].copy_from_slice(&v.to_le_bytes()[..len]);
            dst = &mut dst[len..];
        }
        Ok(())
    }

    pub fn convert_to_ranges(
        &mut self,
        num_symbols: i32,
        p: usize,
        syms: &[u8],
        bits: &mut BitReader,
    ) -> Res<Vec<HuffRange>> {
        let mut sym_idx = 0;
        let mut symlen = num_symbols as usize;

        // Start with space?
        if p & 1 != 0 {
            bits.refill(self).at(self)?;
            let v = syms[symlen] as i32;
            symlen += 1;
            self.assert_lt(v, 8)?;
            sym_idx = bits.read_bits_no_refill(v + 1) + (1 << (v + 1)) - 1;
        }

        let mut syms_used = 0;
        let num_ranges = p >> 1;
        let mut ranges: Vec<HuffRange> = Vec::with_capacity(num_ranges + 1);

        for _ in 0..num_ranges {
            bits.refill(self).at(self)?;
            let v = syms[symlen] as i32;
            symlen += 1;
            self.assert_lt(v, 9)?;
            let num = bits.read_bits_no_refill_zero(v) + (1 << v);
            let v = syms[symlen] as i32;
            symlen += 1;
            self.assert_lt(v, 8)?;
            let space = bits.read_bits_no_refill(v + 1) + (1 << (v + 1)) - 1;
            ranges.push(HuffRange {
                symbol: sym_idx as u16,
                num: num as u16,
            });
            syms_used += num;
            sym_idx += num + space;
        }

        self.assert_lt(sym_idx, 256)?;
        self.assert_lt(syms_used, num_symbols)?;
        self.assert_le(sym_idx + num_symbols - syms_used, 256)?;

        ranges.push(HuffRange {
            symbol: sym_idx as u16,
            num: (num_symbols - syms_used) as u16,
        });

        Ok(ranges)
    }

    fn decode_recursive(
        &mut self,
        src_org: Pointer,
        src_size: usize,
        mut output: Pointer,
        output_size: usize,
        scratch: Pointer,
    ) -> Res<usize> {
        let mut src = src_org;
        let output_end = output + output_size;
        let src_end = src + src_size;

        self.assert_le(6, src_size)?;

        let byte = self.get_byte(src)? as usize;
        let n = byte & 0x7f;
        self.assert_le(2, n)?;

        if (byte & 0x80) == 0 {
            src += 1;
            for _ in 0..n {
                let mut decoded_size = 0;
                let output_size = (output_end - output)?;
                let dec = self
                    .decode_bytes(
                        &mut output,
                        src,
                        src_end,
                        &mut decoded_size,
                        output_size,
                        true,
                        scratch,
                    )
                    .at(self)?;
                output += decoded_size;
                src += dec;
            }
            self.assert_eq(output, output_end)?;
            Ok((src - src_org)?)
        } else {
            let mut decoded_size = 0;
            let dec = self
                .decode_multi_array(
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
                )
                .at(self)?;
            output += decoded_size;
            self.assert_eq(output, output_end)?;
            Ok(dec)
        }
    }

    pub fn decode_multi_array(
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
    ) -> Res<usize> {
        let mut src = src_org;

        self.assert_le(4, (src_end - src)?)?;

        let mut decoded_size = 0;
        let mut num_arrays_in_file = self.get_byte(src)? as usize;
        src += 1;
        self.assert_ne(num_arrays_in_file & 0x80, 0)?;
        num_arrays_in_file &= 0x3f;

        let mut total_size = 0;

        if num_arrays_in_file == 0 {
            for _ in 0..array_count {
                let mut chunk_dst = dst;
                let dec = self
                    .decode_bytes(
                        &mut chunk_dst,
                        src,
                        src_end,
                        &mut decoded_size,
                        (dst_end - dst)?,
                        force_memmove,
                        scratch,
                    )
                    .at(self)?;
                dst += decoded_size;
                array_data.push(chunk_dst);
                array_lens.push(decoded_size);
                src += dec;
                total_size += decoded_size;
            }
            *total_size_out = total_size;
            return Ok((src - src_org)?);
        }

        let mut entropy_array_data = [Default::default(); 63];
        let mut entropy_array_size = [0; 63];

        // First loop just decodes everything to scratch
        let mut scratch_cur = scratch;

        for i in 0..num_arrays_in_file {
            let mut chunk_dst = scratch_cur;
            let dec = self
                .decode_bytes(
                    &mut chunk_dst,
                    src,
                    src_end,
                    &mut decoded_size,
                    usize::MAX,
                    force_memmove,
                    scratch_cur,
                )
                .at(self)?;
            entropy_array_data[i] = chunk_dst;
            entropy_array_size[i] = decoded_size;
            scratch_cur += decoded_size;
            total_size += decoded_size;
            src += dec;
        }
        *total_size_out = total_size;

        self.assert_le(3, (src_end - src)?)?;

        let q = self.get_le_bytes(src, 2).at(self)?;
        src += 2;

        let num_indexes = self.get_block_size(src, src_end, total_size).at(self)?;

        let mut num_lens = num_indexes - array_count;
        self.assert_ne(num_lens, 0)?;

        let mut interval_lenlog2 = scratch_cur;
        scratch_cur += num_indexes;

        let mut interval_indexes = scratch_cur;
        scratch_cur += num_indexes;

        if (q & 0x8000) != 0 {
            let mut size_out = 0;
            let n = self
                .decode_bytes(
                    &mut interval_indexes,
                    src,
                    src_end,
                    &mut size_out,
                    num_indexes,
                    true,
                    scratch_cur,
                )
                .at(self)?;
            self.assert_eq(size_out, num_indexes)?;
            src += n;

            for i in 0..num_indexes {
                let t = self.get_byte(interval_indexes + i)?;
                self.set(interval_lenlog2 + i, t >> 4).at(self)?;
                self.set(interval_indexes + i, t & 0xF).at(self)?;
            }

            num_lens = num_indexes;
        } else {
            let lenlog2_chunksize = num_indexes - array_count;

            let mut size_out = 0;
            let n = self
                .decode_bytes(
                    &mut interval_indexes,
                    src,
                    src_end,
                    &mut size_out,
                    num_indexes,
                    false,
                    scratch_cur,
                )
                .at(self)?;
            self.assert_eq(size_out, num_indexes)?;
            src += n;

            let n = self
                .decode_bytes(
                    &mut interval_lenlog2,
                    src,
                    src_end,
                    &mut size_out,
                    lenlog2_chunksize,
                    false,
                    scratch_cur,
                )
                .at(self)?;
            self.assert_eq(size_out, lenlog2_chunksize)?;
            src += n;

            for i in 0..lenlog2_chunksize {
                self.assert_le(self.get_byte(interval_lenlog2 + i)?, 16)?;
            }
        }

        let mut decoded_intervals = Vec::with_capacity(num_lens);

        let varbits_complen = q & 0x3FFF;
        self.assert_le(varbits_complen, (src_end - src)?)?;

        let mut f = src;
        let mut bits_f = 0u32;
        let mut bitpos_f = 24;

        let src_end_actual = src + varbits_complen;

        let mut b = src_end_actual;
        let mut bits_b = 0u32;
        let mut bitpos_b = 24;

        const BITMASKS: [u32; 32] = [
            0x1, 0x3, 0x7, 0xf, 0x1f, 0x3f, 0x7f, 0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff,
            0x7fff, 0xffff, 0x1ffff, 0x3ffff, 0x7ffff, 0xfffff, 0x1fffff, 0x3fffff, 0x7fffff,
            0xffffff, 0x1ffffff, 0x3ffffff, 0x7ffffff, 0xfffffff, 0x1fffffff, 0x3fffffff,
            0x7fffffff, 0xffffffff,
        ];

        for i in 0..num_lens / 2 {
            bits_f |= self
                .get_be_bytes(f, 4.min(self.input.len() - f.index))
                .at(self)? as u32
                >> (24 - bitpos_f);
            f += (bitpos_f + 7) >> 3;

            bits_b |= self.get_le_bytes((b - 4)?, 4).at(self)? as u32 >> (24 - bitpos_b);
            b -= (bitpos_b + 7) >> 3;

            let numbits_f = self.get_byte(interval_lenlog2 + i * 2 + 0)? as i32;
            let numbits_b = self.get_byte(interval_lenlog2 + i * 2 + 1)? as i32;

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
            bits_f |= self
                .get_be_bytes(f, 4.min(self.input.len() - f.index))
                .at(self)? as u32
                >> (24 - bitpos_f);
            let numbits_f = self.get_byte((interval_lenlog2 + num_lens - 1)?)?;
            bits_f = (bits_f | 1).rotate_left(numbits_f as _);
            let value_f = bits_f & BITMASKS[numbits_f as usize];
            decoded_intervals.push(value_f);
        }

        self.assert_eq(self.get_byte((interval_indexes + num_indexes - 1)?)?, 0)?;

        let mut indi = 0;
        let mut leni = 0;
        let increment_leni = (q & 0x8000) != 0;

        for arri in 0..array_count {
            array_data.push(dst);

            self.assert_lt(indi, num_indexes)?;

            loop {
                let source = self.get_byte(interval_indexes + indi)? as usize;
                indi += 1;
                if source == 0 {
                    break;
                }
                self.assert_le(source, num_arrays_in_file)?;
                self.assert_lt(leni, num_lens)?;
                let cur_len = decoded_intervals[leni] as usize;
                leni += 1;
                let bytes_left = entropy_array_size[source - 1];
                self.assert_le(cur_len, bytes_left)?;
                self.assert_le(cur_len, (dst_end - dst)?)?;
                let blksrc = entropy_array_data[source - 1];
                entropy_array_size[source - 1] -= cur_len;
                entropy_array_data[source - 1] += cur_len;
                self.copy_bytes(dst, blksrc, cur_len).at(self)?;
                dst += cur_len;
            }
            if increment_leni {
                leni += 1;
            }
            array_lens.push((dst - array_data[arri])?);
        }

        self.assert_eq(indi, num_indexes)?;
        self.assert_eq(leni, num_lens)?;

        for &i in entropy_array_size[..num_arrays_in_file].iter() {
            self.assert_eq(i, 0)?
        }

        Ok((src_end_actual - src_org)?)
    }

    fn get_block_size(
        &mut self,
        src_org: Pointer,
        src_end: Pointer,
        dest_capacity: usize,
    ) -> Res<usize> {
        let mut src = src_org;
        let src_size;
        let dst_size;

        let chunk_type = (self.get_byte(src)? >> 4) & 0x7;
        if chunk_type == 0 {
            if self.get_byte(src)? >= 0x80 {
                // In this mode, memcopy stores the length in the bottom 12 bits.
                src_size = self.get_be_bytes(src, 2).at(self)? & 0xFFF;
                src += 2;
            } else {
                src_size = self.get_be_bytes(src, 3).at(self)?;
                self.assert_eq(src_size & !0x3ffff, 0)?; // reserved bits must not be set
                src += 3;
            }
            self.assert_le(src_size, dest_capacity)?;
            self.assert_le(src_size, (src_end - src)?)?;
            return Ok(src_size);
        }

        self.assert_lt(chunk_type, 6)?;

        // In all the other modes, the initial bytes encode
        // the src_size and the dst_size
        if self.get_byte(src)? >= 0x80 {
            // short mode, 10 bit sizes
            let bits = self.get_be_bytes(src, 3).at(self)?;
            src_size = bits & 0x3ff;
            dst_size = src_size + ((bits >> 10) & 0x3ff) + 1;
            src += 3;
        } else {
            // long mode, 18 bit sizes
            // no test coverage
            let bits = self.get_be_bytes(src, 5).at(self)?;
            src_size = bits & 0x3ffff;
            dst_size = (((bits >> 18) | ((self.get_byte(src)? as usize) << 14)) & 0x3FFFF) + 1;
            self.assert_lt(src_size, dst_size)?;
            src += 5;
        }
        self.assert_le(src_size, (src_end - src)?)?;
        self.assert_le(dst_size, dest_capacity)?;
        Ok(dst_size)
    }

    fn decode_rle(
        &mut self,
        src: Pointer,
        src_size: usize,
        mut dst: Pointer,
        dst_size: usize,
        scratch: Pointer,
    ) -> Res<usize> {
        self.assert_ne(src_size, 0)?;
        if src_size == 1 {
            self.memset(dst, self.get_byte(src)?, dst_size).at(self)?;
            return Ok(1);
        }
        let dst_end = dst + dst_size;
        let mut cmd_ptr = src + 1;
        let mut cmd_ptr_end = src + src_size;
        // Unpack the first X bytes of the command buffer?
        if self.get_byte(src)? != 0 {
            let mut dst_ptr = scratch;
            let mut dec_size = 0;
            let n = self
                .decode_bytes(
                    &mut dst_ptr,
                    src,
                    src + src_size,
                    &mut dec_size,
                    usize::MAX,
                    true,
                    scratch,
                )
                .at(self)?;
            self.assert_lt(0, n)?;
            let cmd_len = src_size - n + dec_size;
            self.copy_bytes(dst_ptr + dec_size, src + n, src_size - n)
                .at(self)?;
            cmd_ptr = dst_ptr;
            cmd_ptr_end = dst_ptr + cmd_len;
        }

        let mut rle_byte = 0;

        while cmd_ptr < cmd_ptr_end {
            let cmd = self.get_byte((cmd_ptr_end - 1)?)? as usize;
            if cmd == 0 || cmd > 0x2f {
                cmd_ptr_end -= 1;
                let bytes_to_copy = !cmd & 0xF;
                let bytes_to_rle = cmd >> 4;
                self.assert_le(bytes_to_copy + bytes_to_rle, (dst_end - dst)?)?;
                self.assert_le(bytes_to_copy, (cmd_ptr_end - cmd_ptr)?)?;
                self.copy_bytes(dst, cmd_ptr, bytes_to_copy).at(self)?;
                cmd_ptr += bytes_to_copy;
                dst += bytes_to_copy;
                self.memset(dst, rle_byte, bytes_to_rle).at(self)?;
                dst += bytes_to_rle;
            } else if cmd >= 0x10 {
                cmd_ptr_end -= 2;
                let data = self.get_le_bytes(cmd_ptr_end, 2).at(self)? - 4096;
                let bytes_to_copy = data & 0x3F;
                let bytes_to_rle = data >> 6;
                self.assert_le(bytes_to_copy + bytes_to_rle, (dst_end - dst)?)?;
                self.assert_le(bytes_to_copy, (cmd_ptr_end - cmd_ptr)?)?;
                self.copy_bytes(dst, cmd_ptr, bytes_to_copy).at(self)?;
                cmd_ptr += bytes_to_copy;
                dst += bytes_to_copy;
                self.memset(dst, rle_byte, bytes_to_rle).at(self)?;
                dst += bytes_to_rle;
            } else if cmd == 1 {
                rle_byte = self.get_byte(cmd_ptr)?;
                cmd_ptr += 1;
                cmd_ptr_end -= 1;
            } else if cmd >= 9 {
                cmd_ptr_end -= 2;
                let bytes_to_rle = (self.get_le_bytes(cmd_ptr_end, 2).at(self)? - 0x8ff) * 128;
                self.assert_le(bytes_to_rle, (dst_end - dst)?)?;
                self.memset(dst, rle_byte, bytes_to_rle).at(self)?;
                dst += bytes_to_rle;
            } else {
                cmd_ptr_end -= 2;
                let bytes_to_copy = (self.get_le_bytes(cmd_ptr_end, 2).at(self)? - 511) * 64;
                self.assert_le(bytes_to_copy, (cmd_ptr_end - cmd_ptr)?)?;
                self.assert_le(bytes_to_copy, (dst_end - dst)?)?;
                self.copy_bytes(dst, cmd_ptr, bytes_to_copy).at(self)?;
                dst += bytes_to_copy;
                cmd_ptr += bytes_to_copy;
            }
        }

        self.assert_eq(cmd_ptr, cmd_ptr_end)?;
        self.assert_eq(dst, dst_end)?;

        Ok(src_size)
    }

    fn decode_tans(
        &mut self,
        mut src: Pointer,
        src_size: usize,
        dst: Pointer,
        dst_size: usize,
    ) -> Res<usize> {
        self.assert_le(8, src_size)?;
        self.assert_le(5, dst_size)?;

        let mut src_end = src + src_size;

        let mut br = BitReader {
            bitpos: 24,
            bits: 0,
            p: src,
            p_end: src_end,
        };
        br.refill(self).at(self)?;

        self.assert(!br.read_bit_no_refill(), "reserved bit")?;

        let l_bits = br.read_bits_no_refill(2) + 8;

        let mut decoder = TansDecoder::default();
        let tans_data = decoder.decode_table(self, &mut br, l_bits).at(self)?;

        src = (br.p - (24 - br.bitpos) / 8)?;

        self.assert_lt(src, src_end)?;

        decoder.dst = dst;
        decoder.dst_end = (dst + dst_size - 5)?;

        decoder.lut = decoder.init_lut(&tans_data, l_bits);

        // Read out the initial state
        let l_mask = (1 << l_bits) - 1;
        let mut bits_f = self.get_le_bytes(src, 4).at(self)?;
        src += 4;
        src_end -= 4;
        let mut bits_b = self.get_be_bytes(src_end, 4).at(self)?;
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
        bits_f |= self.get_le_bytes(src, 4).at(self)? << bitpos_f;
        src += (31 - bitpos_f) >> 3;
        bitpos_f |= 24;

        // Read final state variable
        decoder.state[4] = bits_f & l_mask;
        bits_f >>= l_bits;
        bitpos_f -= l_bits;

        decoder.bits_f = bits_f;
        decoder.ptr_f = (src - (bitpos_f >> 3))?;
        decoder.bitpos_f = (bitpos_f & 7) as _;

        decoder.bits_b = bits_b;
        decoder.ptr_b = src_end + (bitpos_b >> 3);
        decoder.bitpos_b = (bitpos_b & 7) as _;

        decoder.decode(self).at(self)?;

        Ok(src_size)
    }
}

impl ErrorContext for Core<'_> {
    fn describe(&self) -> Option<String> {
        Some(format!(
            "Source index: {}, destination index: {}",
            self.src.index, self.dst.index
        ))
    }
}
