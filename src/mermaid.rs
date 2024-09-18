use crate::algorithm::Algorithm;
use crate::core::Core;
use crate::pointer::Pointer;
use std::collections::VecDeque;

#[derive(Debug)]
pub struct Mermaid;

impl Algorithm for Mermaid {
    fn process(
        &self,
        core: &mut Core,
        mode: usize,
        src: Pointer,
        src_used: usize,
        dst_start: Pointer,
        dst: Pointer,
        dst_size: usize,
    ) {
        let mut lz = MermaidLzTable::read_lz_table(
            core,
            mode,
            src,
            src + src_used,
            dst,
            dst_size,
            dst - dst_start,
        );
        lz.process_lz_runs(core, mode, src + src_used, dst, dst_size, dst - dst_start);
    }
}

#[derive(Default, Copy, Clone)]
enum Chunk {
    #[default]
    Stream1,
    Stream2,
}

/// Mermaid/Selkie decompression also happens in two phases, just like in Kraken,
/// but the match copier works differently.
/// Both Mermaid and Selkie use the same on-disk format, only the compressor
/// differs.
#[derive(Default)]
struct MermaidLzTable {
    // Flag stream. Format of flags:
    // Read flagbyte from |cmd_stream|
    // If flagbyte >= 24:
    //   flagbyte & 0x80 == 0 : Read from |off16_stream| into |recent_offs|.
    //                   != 0 : Don't read offset.
    //   flagbyte & 7 = Number of literals to copy first from |lit_stream|.
    //   (flagbyte >> 3) & 0xF = Number of bytes to copy from |recent_offs|.
    //
    //  If flagbyte == 0 :
    //    Read byte L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 64
    //    Copy L bytes from |lit_stream|.
    //
    //  If flagbyte == 1 :
    //    Read byte L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 91
    //    Copy L bytes from match pointed by next offset from |off16_stream|
    //
    //  If flagbyte == 2 :
    //    Read byte L from |length_stream|
    //    If L > 251: L += 4 * Read word from |length_stream|
    //    L += 29
    //    Copy L bytes from match pointed by next offset from |off32_stream|,
    //    relative to start of block.
    //    Then prefetch |off32_stream[3]|
    //
    //  If flagbyte > 2:
    //    L = flagbyte + 5
    //    Copy L bytes from match pointed by next offset from |off32_stream|,
    //    relative to start of block.
    //    Then prefetch |off32_stream[3]|
    cmd_stream: Pointer,
    cmd_stream_end: Pointer,

    /// Length stream
    length_stream: Pointer,

    /// Literal stream
    lit_stream: Pointer,
    lit_stream_end: Pointer,

    /// Near offsets
    off16_stream: VecDeque<u16>,

    /// Stream selector for current chunk
    off32_stream: Chunk,

    /// Holds the offsets for the two chunks
    off32_stream_1: Vec<u32>,
    off32_stream_2: Vec<u32>,

    /// Flag offsets for next 64k chunk.
    cmd_stream_2_offs: usize,
    cmd_stream_2_offs_end: usize,
}

impl MermaidLzTable {
    pub(crate) fn process_lz_runs(
        &mut self,
        core: &mut Core,
        mode: usize,
        src_end: Pointer,
        mut dst: Pointer,
        mut dst_size: usize,
        offset: usize,
    ) {
        let mut saved_dist = -8;

        for iteration in 0..2 {
            let mut dst_size_cur = dst_size;
            if dst_size_cur > 0x10000 {
                dst_size_cur = 0x10000;
            }

            if iteration == 0 {
                self.off32_stream = Chunk::Stream1;
                self.cmd_stream_end = self.cmd_stream + self.cmd_stream_2_offs;
            } else {
                self.off32_stream = Chunk::Stream2;
                self.cmd_stream_end = self.cmd_stream + self.cmd_stream_2_offs_end;
                self.cmd_stream += self.cmd_stream_2_offs;
            }
            let startoff = if (offset == 0) && (iteration == 0) {
                8
            } else {
                0
            };

            if mode == 0 {
                self.process::<true>(core, dst, dst_size_cur, src_end, &mut saved_dist, startoff);
            } else {
                self.process::<false>(core, dst, dst_size_cur, src_end, &mut saved_dist, startoff);
            }
            assert!(!self.length_stream.is_null());

            dst += dst_size_cur;
            dst_size -= dst_size_cur;
            if dst_size == 0 {
                break;
            }
        }

        assert_eq!(self.length_stream, src_end);
    }

    fn process<const ADD_MODE: bool>(
        &mut self,
        core: &mut Core,
        mut dst: Pointer,
        dst_size: usize,
        src_end: Pointer,
        saved_dist: &mut i32,
        startoff: i32,
    ) {
        let dst_end = dst + dst_size;
        let mut cmd_stream = self.cmd_stream;
        let cmd_stream_end = self.cmd_stream_end;
        let mut length_stream = self.length_stream;
        let mut lit_stream = self.lit_stream;
        let lit_stream_end = self.lit_stream_end;
        let off32 = match self.off32_stream {
            Chunk::Stream1 => &self.off32_stream_1,
            Chunk::Stream2 => &self.off32_stream_2,
        };
        let mut off32_stream = 0;
        let off32_stream_end = off32.len();
        let mut recent_offs = *saved_dist;
        let mut offs_ptr;
        let mut length;
        let dst_begin = dst;

        dst += startoff;

        while cmd_stream < cmd_stream_end {
            let cmd = core.get_as_usize(cmd_stream);
            cmd_stream += 1;
            if cmd >= 24 {
                let litlen = cmd & 7;
                if ADD_MODE {
                    core.copy_64_add(dst, lit_stream, dst + recent_offs, litlen);
                } else {
                    core.repeat_copy_64(dst, lit_stream, litlen);
                }
                dst += litlen;
                lit_stream += litlen;
                if (cmd >> 7) == 0 {
                    recent_offs = -(self.off16_stream.pop_front().unwrap() as i32);
                }
                offs_ptr = dst + recent_offs;
                core.repeat_copy_64(dst, offs_ptr, (cmd >> 3) & 0xF);
                dst += (cmd >> 3) & 0xF;
            } else if cmd > 2 {
                length = cmd + 5;

                assert_ne!(off32_stream, off32_stream_end);
                offs_ptr = dst_begin - off32[off32_stream];
                off32_stream += 1;
                recent_offs = offs_ptr.index as i32 - dst.index as i32;

                assert!(dst_end - dst >= length);
                core.repeat_copy_64(dst, offs_ptr, length);
                dst += length;
                //simde_mm_prefetch((char*)dst_begin - off32_stream[3], SIMDE_MM_HINT_T0);
            } else if cmd == 0 {
                assert_ne!(src_end - length_stream, 0);
                length = core.get_as_usize(length_stream);
                if length > 251 {
                    assert!(src_end - length_stream >= 3);
                    length += core.get_bytes_as_usize_le(length_stream + 1, 2) * 4;
                    length_stream += 2;
                }
                length_stream += 1;

                length += 64;
                assert!(dst_end - dst >= length);
                assert!(lit_stream_end - lit_stream >= length);
                if ADD_MODE {
                    core.copy_64_add(dst, lit_stream, dst + recent_offs, length);
                } else {
                    core.repeat_copy_64(dst, lit_stream, length);
                }
                dst += length;
                lit_stream += length;
            } else if cmd == 1 {
                assert_ne!(src_end - length_stream, 0);
                length = core.get_as_usize(length_stream);
                if length > 251 {
                    assert!(src_end - length_stream >= 3);
                    length += core.get_bytes_as_usize_le(length_stream + 1, 2) * 4;
                    length_stream += 2;
                }
                length_stream += 1;
                length += 91;

                offs_ptr = dst - self.off16_stream.pop_front().unwrap() as usize;
                recent_offs = offs_ptr.index as i32 - dst.index as i32;
                core.repeat_copy_64(dst, offs_ptr, length);
                dst += length;
            } else {
                /* flag == 2 */
                assert_ne!(src_end - length_stream, 0);
                length = core.get_as_usize(length_stream);
                if length > 251 {
                    assert!(src_end - length_stream >= 3);
                    length += core.get_bytes_as_usize_le(length_stream + 1, 2) * 4;
                    length_stream += 2;
                }
                length_stream += 1;
                length += 29;
                assert_ne!(off32_stream, off32_stream_end);
                offs_ptr = dst_begin - off32[off32_stream];
                off32_stream += 1;
                recent_offs = offs_ptr.index as i32 - dst.index as i32;
                core.repeat_copy_64(dst, offs_ptr, length);
                dst += length;
                //simde_mm_prefetch((char*)dst_begin - off32_stream[3], SIMDE_MM_HINT_T0);
            }
        }

        length = dst_end - dst;
        if ADD_MODE {
            core.copy_64_add(dst, lit_stream, dst + recent_offs, length);
        } else {
            core.repeat_copy_64(dst, lit_stream, length);
        }
        lit_stream += length;

        *saved_dist = recent_offs;
        self.length_stream = length_stream;
        self.lit_stream = lit_stream;
    }
}

impl MermaidLzTable {
    fn read_lz_table(
        core: &mut Core,
        mode: usize,
        mut src: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> MermaidLzTable {
        let mut out;
        let mut decode_count = 0;
        let mut off32_size_2;
        let mut off32_size_1;
        let mut scratch = Pointer::tmp(0);
        let mut lz = MermaidLzTable::default();

        assert!(mode <= 1, "{}", mode);
        assert!(src_end - src >= 10);

        if offset == 0 {
            core.memmove(dst, src, 8);
            dst += 8;
            src += 8;
        }

        // Decode lit stream
        out = scratch;
        src += core.decode_bytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            dst_size,
            false,
            Pointer::scratch(0),
        );
        lz.lit_stream = out;
        lz.lit_stream_end = out + decode_count;
        scratch += decode_count;

        // Decode flag stream
        out = scratch;
        src += core.decode_bytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            dst_size,
            false,
            Pointer::scratch(0),
        );
        lz.cmd_stream = out;
        lz.cmd_stream_end = out + decode_count;
        scratch += decode_count;

        lz.cmd_stream_2_offs_end = decode_count;
        if dst_size <= 0x10000 {
            lz.cmd_stream_2_offs = decode_count;
        } else {
            assert!(src_end - src >= 2);
            lz.cmd_stream_2_offs = core.get_bytes_as_usize_le(src, 2);
            src += 2;
            assert!(lz.cmd_stream_2_offs <= lz.cmd_stream_2_offs_end);
        }

        assert!(src_end - src >= 2);

        let off16_count = core.get_bytes_as_usize_le(src, 2);
        src += 2;
        if off16_count == 0xffff {
            // off16 is entropy coded
            let mut off16_lo;
            let mut off16_hi;
            let mut off16_lo_count = 0;
            let mut off16_hi_count = 0;
            off16_hi = scratch;
            src += core.decode_bytes(
                &mut off16_hi,
                src,
                src_end,
                &mut off16_hi_count,
                dst_size >> 1,
                false,
                Pointer::scratch(0),
            );
            scratch += off16_hi_count;

            off16_lo = scratch;
            src += core.decode_bytes(
                &mut off16_lo,
                src,
                src_end,
                &mut off16_lo_count,
                dst_size >> 1,
                false,
                Pointer::scratch(0),
            );
            scratch += off16_lo_count;

            assert_eq!(off16_lo_count, off16_hi_count);
            lz.off16_stream.reserve(off16_lo_count);
            for i in 0..off16_lo_count {
                lz.off16_stream.push_back(
                    core.get_byte(off16_lo + i) as u16 + core.get_byte(off16_hi + i) as u16 * 256,
                )
            }
        } else {
            lz.off16_stream = core
                .get_slice(src, off16_count * 2)
                .chunks(2)
                .map(|c| u16::from_le_bytes(c.try_into().unwrap()))
                .collect();
            src += off16_count * 2;
        }

        assert!(src_end - src >= 3);
        let tmp = core.get_bytes_as_usize_le(src, 3);
        src += 3;

        if tmp != 0 {
            off32_size_1 = tmp >> 12;
            off32_size_2 = tmp & 0xFFF;
            if off32_size_1 == 4095 {
                assert!(src_end - src >= 2);
                off32_size_1 = core.get_bytes_as_usize_le(src, 2);
                src += 2;
            }
            if off32_size_2 == 4095 {
                assert!(src_end - src >= 2);
                off32_size_2 = core.get_bytes_as_usize_le(src, 2);
                src += 2;
            }

            lz.off32_stream_1.reserve(off32_size_1);
            // store dummy bytes after for simde_mm_prefetch.
            // ((uint64*)scratch)[0] = 0;
            // ((uint64*)scratch)[1] = 0;
            // ((uint64*)scratch)[2] = 0;
            // ((uint64*)scratch)[3] = 0;

            lz.off32_stream_2.reserve(off32_size_2);
            // store dummy bytes after for simde_mm_prefetch.
            // ((uint64*)scratch)[0] = 0;
            // ((uint64*)scratch)[1] = 0;
            // ((uint64*)scratch)[2] = 0;
            // ((uint64*)scratch)[3] = 0;

            src += MermaidLzTable::decode_far_offsets(
                core,
                src,
                src_end,
                &mut lz.off32_stream_1,
                off32_size_1,
                offset,
            );

            src += MermaidLzTable::decode_far_offsets(
                core,
                src,
                src_end,
                &mut lz.off32_stream_2,
                off32_size_2,
                offset + 0x10000,
            );
        }
        lz.length_stream = src;

        lz
    }

    fn decode_far_offsets(
        core: &mut Core,
        src: Pointer,
        src_end: Pointer,
        output: &mut Vec<u32>,
        output_size: usize,
        offset: usize,
    ) -> usize {
        let mut src_cur = src;

        if offset < (0xC00000 - 1) {
            for _ in 0..output_size {
                assert!(src_end - src_cur >= 3);
                let off = core.get_bytes_as_usize_le(src_cur, 3);
                src_cur += 3;
                assert!(off <= offset);
                output.push(off as u32);
            }
            src_cur - src
        } else {
            for _ in 0..output_size {
                assert!(src_end - src_cur >= 3);
                let mut off = core.get_bytes_as_usize_le(src_cur, 3);
                src_cur += 3;

                if off >= 0xc00000 {
                    assert_ne!(src_cur, src_end);
                    off += core.get_as_usize(src_cur) << 22;
                    src_cur += 1;
                }
                assert!(off <= offset);
                output.push(off as u32);
            }
            src_cur - src
        }
    }
}
