use crate::algorithm::Algorithm;
use crate::core::Core;
use crate::pointer::{IntPointer, Pointer};

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

#[derive(Debug)]
pub struct Kraken;

impl Algorithm for Kraken {
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
        assert!(mode <= 1);
        let mut lz = KrakenLzTable::Kraken_ReadLzTable(
            core,
            src,
            src + src_used,
            dst,
            dst_size,
            dst - dst_start,
        );
        lz.Kraken_ProcessLzRuns(core, mode, dst, dst_size, dst - dst_start)
    }
}

impl KrakenLzTable {
    fn Kraken_ReadLzTable(
        core: &mut Core,
        mut src: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> KrakenLzTable {
        let mut out;
        let mut decode_count = 0;
        let mut n;
        let mut packed_offs_stream;
        let mut packed_len_stream;
        let mut scratch = Pointer::scratch(0);

        assert!(src_end - src >= 13, "{:?} {:?}", src_end, src);

        if offset == 0 {
            core.memmove(dst, src, 8);
            dst += 8;
            src += 8;
        }

        if core.get_as_usize(src) & 0x80 != 0 {
            let flag = core.get_as_usize(src);
            src += 1;
            assert_eq!(flag & 0xc0, 0x80, "reserved flag set");
            // fail anyway...
            panic!("excess bytes not supported");
        }

        // Disable no copy optimization if source and dest overlap
        let force_copy = dst <= src_end && src <= dst + dst_size;

        // Decode lit stream, bounded by dst_size
        out = scratch;
        n = core.Kraken_DecodeBytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            dst_size,
            force_copy,
            scratch,
        );
        src += n;
        let mut lz = KrakenLzTable {
            lit_stream: out,
            lit_stream_size: decode_count,
            ..Default::default()
        };
        scratch += decode_count;

        // Decode command stream, bounded by dst_size
        out = scratch;
        n = core.Kraken_DecodeBytes(
            &mut out,
            src,
            src_end,
            &mut decode_count,
            dst_size,
            force_copy,
            scratch,
        );
        src += n;
        lz.cmd_stream = out;
        lz.cmd_stream_size = decode_count;
        scratch += decode_count;

        // Check if to decode the multistuff crap
        assert!(src_end - src >= 3, "{:?} {:?}", src_end, src);

        let mut offs_scaling = 0;
        let mut packed_offs_stream_extra = Pointer::null();

        if core.get_as_usize(src) & 0x80 != 0 {
            // uses the mode where distances are coded with 2 tables
            offs_scaling = i32::from(core.get_byte(src)) - 127;
            src += 1;

            packed_offs_stream = scratch;
            n = core.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lz.offs_stream_size,
                lz.cmd_stream_size,
                false,
                scratch,
            );
            src += n;
            scratch += lz.offs_stream_size;

            if offs_scaling != 1 {
                packed_offs_stream_extra = scratch;
                n = core.Kraken_DecodeBytes(
                    &mut packed_offs_stream_extra,
                    src,
                    src_end,
                    &mut decode_count,
                    lz.offs_stream_size,
                    false,
                    scratch,
                );
                assert_eq!(decode_count, lz.offs_stream_size);
                src += n;
                scratch += decode_count;
            }
        } else {
            // Decode packed offset stream, it's bounded by the command length.
            packed_offs_stream = scratch;
            n = core.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lz.offs_stream_size,
                lz.cmd_stream_size,
                false,
                scratch,
            );
            src += n;
            scratch += lz.offs_stream_size;
        }

        // Decode packed litlen stream. It's bounded by 1/4 of dst_size.
        packed_len_stream = scratch;
        n = core.Kraken_DecodeBytes(
            &mut packed_len_stream,
            src,
            src_end,
            &mut lz.len_stream_size,
            dst_size >> 2,
            false,
            scratch,
        );
        src += n;
        scratch += lz.len_stream_size;

        // Reserve memory for final dist stream
        scratch = scratch.align(16);
        lz.offs_stream = scratch.into();
        scratch += lz.offs_stream_size * 4;

        // Reserve memory for final len stream
        scratch = scratch.align(16);
        lz.len_stream = scratch.into();
        scratch += lz.len_stream_size * 4;

        core.Kraken_UnpackOffsets(
            src,
            src_end,
            packed_offs_stream,
            packed_offs_stream_extra,
            lz.offs_stream_size,
            offs_scaling,
            packed_len_stream,
            lz.len_stream_size,
            lz.offs_stream,
            lz.len_stream,
            false,
        );

        lz
    }

    fn Kraken_ProcessLzRuns(
        &mut self,
        core: &mut Core,
        mode: usize,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) {
        let dst_end = dst + dst_size;
        if offset == 0 {
            dst += 8
        };

        let mut cmd_stream = self.cmd_stream;
        let cmd_stream_end = cmd_stream + self.cmd_stream_size;
        let mut len_stream = self.len_stream;
        let len_stream_end = self.len_stream + self.len_stream_size;
        let mut lit_stream = self.lit_stream;
        let lit_stream_end = self.lit_stream + self.lit_stream_size;
        let mut offs_stream = self.offs_stream;
        let offs_stream_end = self.offs_stream + self.offs_stream_size;
        let mut copyfrom;
        let mut offset;
        let mut recent_offs = [0; 7];
        let mut last_offset: i32;

        recent_offs[3] = -8;
        recent_offs[4] = -8;
        recent_offs[5] = -8;
        last_offset = -8;

        while cmd_stream < cmd_stream_end {
            let f = core.get_as_usize(cmd_stream);
            cmd_stream += 1;
            let mut litlen = f & 3;
            let offs_index = f >> 6;
            let mut matchlen = (f >> 2) & 0xF;

            // use cmov
            let next_long_length = core.get_int(len_stream);
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
            recent_offs[6] = core.get_int(offs_stream);

            if mode == 0 {
                core.copy_64_add(dst, lit_stream, dst + last_offset, litlen);
            } else {
                core.memmove(dst, lit_stream, litlen);
            }
            dst += litlen;
            lit_stream += litlen;

            offset = recent_offs[offs_index + 3];
            recent_offs.copy_within(offs_index..offs_index + 3, offs_index + 1);
            recent_offs[3] = offset;
            last_offset = offset;

            if offs_index == 3 {
                offs_stream += 1;
            }

            copyfrom = dst + offset;
            if matchlen != 15 {
                core.repeat_copy_64(dst, copyfrom, matchlen + 2);
                dst += matchlen + 2;
            } else {
                // why is the value not 16 here, the above case copies up to 16 bytes.
                matchlen = (14 + core.get_int(len_stream)).try_into().unwrap();
                len_stream += 1;
                core.repeat_copy_64(dst, copyfrom, matchlen);
                dst += matchlen;
            }
        }

        // check for incorrect input
        assert_eq!(offs_stream, offs_stream_end);
        assert_eq!(len_stream, len_stream_end);

        let final_len = dst_end - dst;
        assert_eq!(final_len, lit_stream_end - lit_stream);

        if mode == 0 {
            core.copy_64_add(dst, lit_stream, dst + last_offset, final_len);
        } else {
            core.memmove(dst, lit_stream, final_len);
        }
    }
}
