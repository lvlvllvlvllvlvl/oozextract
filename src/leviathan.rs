use crate::algorithm::Algorithm;
use crate::core::Core;
use crate::pointer::{IntPointer, Pointer};

#[derive(Default)]
pub struct LeviathanLzTable {
    offs_stream: IntPointer,
    offs_stream_size: usize,
    len_stream: IntPointer,
    len_stream_size: usize,
    lit_stream: Vec<Pointer>,
    lit_stream_size: Vec<usize>,
    lit_stream_total: usize,
    multi_cmd_ptr: Vec<Pointer>,
    multi_cmd_end: Vec<usize>,
    cmd_stream: Pointer,
    cmd_stream_size: usize,
}

#[derive(Debug)]
pub struct Leviathan;

impl Algorithm for Leviathan {
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
        let lz = LeviathanLzTable::Leviathan_ReadLzTable(
            core,
            mode,
            src,
            src + src_used,
            dst,
            dst_size,
            dst - dst_start,
        );
        lz.Leviathan_ProcessLzRuns(core, mode, dst, dst_size, dst - dst_start);
    }
}

impl LeviathanLzTable {
    fn Leviathan_ReadLzTable(
        core: &mut Core,
        chunk_type: usize,
        mut src: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> LeviathanLzTable {
        let mut scratch = Pointer::scratch(0);
        let mut packed_offs_stream;
        let mut packed_len_stream;
        let mut out;
        let mut decode_count = 0;

        assert!(chunk_type <= 5, "invalid chunk type {}", chunk_type);
        assert!(src_end - src >= 13, "{}", src_end - src);

        if offset == 0 {
            core.memmove(dst, src, 8);
            dst += 8;
            src += 8;
        }

        let mut offs_scaling = 0;
        let mut packed_offs_stream_extra = Pointer::null();
        let offs_stream_limit = dst_size / 3;
        let mut lztable = LeviathanLzTable::default();

        if core.get_byte(src) & 0x80 == 0 {
            // Decode packed offset stream, it's bounded by the command length.
            packed_offs_stream = scratch;
            src += core.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lztable.offs_stream_size,
                offs_stream_limit,
                false,
                scratch,
            );
            scratch += lztable.offs_stream_size;
        } else {
            // uses the mode where distances are coded with 2 tables
            // and the transformation offs * scaling + low_bits
            offs_scaling = core.get_byte(src) as i32 - 127;
            src += 1;

            packed_offs_stream = scratch;
            src += core.Kraken_DecodeBytes(
                &mut packed_offs_stream,
                src,
                src_end,
                &mut lztable.offs_stream_size,
                offs_stream_limit,
                false,
                scratch,
            );
            scratch += lztable.offs_stream_size;

            if offs_scaling != 1 {
                packed_offs_stream_extra = scratch;
                src += core.Kraken_DecodeBytes(
                    &mut packed_offs_stream_extra,
                    src,
                    src_end,
                    &mut decode_count,
                    offs_stream_limit,
                    false,
                    scratch,
                );
                assert_eq!(decode_count, lztable.offs_stream_size);
                scratch += decode_count;
            }
        }

        // Decode packed litlen stream. It's bounded by 1/5 of dst_size.
        packed_len_stream = scratch;
        src += core.Kraken_DecodeBytes(
            &mut packed_len_stream,
            src,
            src_end,
            &mut lztable.len_stream_size,
            dst_size / 5,
            false,
            scratch,
        );
        scratch += lztable.len_stream_size;

        // Reserve memory for final dist stream
        scratch = scratch.align(16);
        lztable.offs_stream = scratch.into();
        scratch += lztable.offs_stream_size * 4;

        // Reserve memory for final len stream
        scratch = scratch.align(16);
        lztable.len_stream = scratch.into();
        scratch += lztable.len_stream_size * 4;

        if chunk_type <= 1 {
            // Decode lit stream, bounded by dst_size
            out = scratch;
            src += core.Kraken_DecodeBytes(
                &mut out,
                src,
                src_end,
                &mut decode_count,
                dst_size,
                true,
                scratch,
            );
            lztable.lit_stream[0] = out;
            lztable.lit_stream_size[0] = decode_count;
        } else {
            let array_count = if chunk_type == 2 {
                2
            } else if chunk_type == 3 {
                4
            } else {
                16
            };
            src += core.Kraken_DecodeMultiArray(
                src,
                src_end,
                scratch,
                Pointer::scratch(usize::MAX),
                &mut lztable.lit_stream,
                &mut lztable.lit_stream_size,
                array_count,
                &mut decode_count,
                true,
                scratch,
            );
        }
        scratch += decode_count;
        lztable.lit_stream_total = decode_count;

        assert!(src < src_end);

        if (core.get_byte(src) & 0x80) == 0 {
            // Decode command stream, bounded by dst_size
            out = scratch;
            src += core.Kraken_DecodeBytes(
                &mut out,
                src,
                src_end,
                &mut decode_count,
                dst_size,
                true,
                scratch,
            );
            lztable.cmd_stream = out;
            lztable.cmd_stream_size = decode_count;
            scratch += decode_count;
        } else {
            assert_eq!(core.get_byte(src), 0x83);
            src += 1;
            src += core.Kraken_DecodeMultiArray(
                src,
                src_end,
                scratch,
                Pointer::scratch(usize::MAX),
                &mut lztable.multi_cmd_ptr,
                &mut lztable.multi_cmd_end,
                8,
                &mut decode_count,
                true,
                scratch,
            );

            lztable.cmd_stream = Pointer::null();
            lztable.cmd_stream_size = decode_count;
            scratch += decode_count;
        }

        core.Kraken_UnpackOffsets(
            src,
            src_end,
            packed_offs_stream,
            packed_offs_stream_extra,
            lztable.offs_stream_size,
            offs_scaling,
            packed_len_stream,
            lztable.len_stream_size,
            lztable.offs_stream,
            lztable.len_stream,
            false,
        );

        lztable
    }
    pub fn Leviathan_ProcessLzRuns(
        &self,
        core: &mut Core,
        mode: usize,
        dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) {
        let dst_cur = if offset == 0 { dst + 8 } else { dst };
        let dst_end = dst + dst_size;
        let dst_start = dst - offset;
        match mode {
            0 => self.process_lz::<LeviathanModeSub>(core, dst_cur, dst, dst_end, dst_start),
            1 => self.process_lz::<LeviathanModeRaw>(core, dst_cur, dst, dst_end, dst_start),
            2 => self.process_lz::<LeviathanModeLamSub>(core, dst_cur, dst, dst_end, dst_start),
            3 => self.process_lz::<LeviathanModeSubAnd<4>>(core, dst_cur, dst, dst_end, dst_start),
            4 => self.process_lz::<LeviathanModeO1>(core, dst_cur, dst, dst_end, dst_start),
            5 => self.process_lz::<LeviathanModeSubAnd<16>>(core, dst_cur, dst, dst_end, dst_start),
            _ => panic!(),
        }
    }
    pub fn process_lz<Mode: LeviathanMode>(
        &self,
        core: &mut Core,
        mut dst: Pointer,
        dst_start: Pointer,
        dst_end: Pointer,
        window_base: Pointer,
    ) {
        let MultiCmd = self.cmd_stream.is_null();
        let mut cmd_stream = self.cmd_stream;
        let cmd_stream_end = cmd_stream + self.cmd_stream_size;
        let mut len_stream = self.len_stream;
        let mut len_stream_end = len_stream + self.len_stream_size;

        let mut offs_stream = self.offs_stream;
        let offs_stream_end = offs_stream + self.offs_stream_size;
        let mut copyfrom;
        let match_zone_end = if dst_end - dst_start >= 16 {
            dst_end - 16
        } else {
            dst_start
        };

        let mut recent_offs: [i32; 16] = core::array::from_fn(|i| match i {
            8..=14 => -8,
            _ => 0,
        });

        let mut offset = -8isize as usize;

        let mut mode = Mode::new(self, dst_start, core);

        let mut cmd_stream_left = 0;
        let mut multi_cmd_stream = [Pointer::null(); 8];
        let mut cmd_stream_ptr;
        if MultiCmd {
            let base = !dst_start.index + 1;
            for (i, p) in multi_cmd_stream.iter_mut().enumerate() {
                *p = self.multi_cmd_ptr[(base + i) & 7];
            }
            cmd_stream_left = self.cmd_stream_size;
            cmd_stream_ptr = multi_cmd_stream[dst.index & 7];
            cmd_stream = cmd_stream_ptr;
        }

        loop {
            let cmd;

            if !MultiCmd {
                if cmd_stream >= cmd_stream_end {
                    break;
                }
                cmd = core.get_as_usize(cmd_stream);
                cmd_stream += 1;
            } else {
                if cmd_stream_left == 0 {
                    break;
                }
                cmd_stream_left -= 1;
                cmd = core.get_as_usize(cmd_stream);
            }

            let offs_index = cmd >> 5;
            assert!(offs_index < 8);
            let mut matchlen = (cmd & 7) + 2;

            recent_offs[15] = core.get_int(offs_stream);

            mode.CopyLiterals(core, cmd, &mut dst, &mut len_stream, match_zone_end, offset);

            offset = recent_offs[offs_index + 8] as usize;

            // Permute the recent offsets table
            let mut temp = [0; 4];
            temp.copy_from_slice(&recent_offs[offs_index + 4..][..4]);
            recent_offs.copy_within(offs_index..offs_index + 4, offs_index + 1);
            recent_offs[offs_index + 5..][..4].copy_from_slice(&temp);
            recent_offs[8] = offset as i32;
            if offs_index == 7 {
                offs_stream += 1;
            }

            assert!(
                offset >= (window_base - dst),
                "offset out of bounds {} {}",
                offset,
                window_base - dst
            );
            copyfrom = dst + offset;

            if matchlen == 9 {
                assert!(len_stream < len_stream_end, "len stream empty");
                len_stream_end = len_stream_end - 1;
                matchlen = (core.get_int(len_stream_end) + 6) as usize;
                core.repeat_copy_64(dst, copyfrom, 16);
                let next_dst = dst + matchlen;
                if MultiCmd {
                    cmd_stream_ptr = multi_cmd_stream[next_dst.index & 7];
                    cmd_stream = cmd_stream_ptr;
                }
                if matchlen > 16 {
                    assert!(matchlen <= (dst_end - 8 - dst), "no space in buf");
                    core.repeat_copy_64(dst, copyfrom, matchlen);
                }
                dst = next_dst;
            } else {
                core.repeat_copy_64(dst, copyfrom, 8);
                dst += matchlen;
                if MultiCmd {
                    cmd_stream_ptr = multi_cmd_stream[dst.index & 7];
                    cmd_stream = cmd_stream_ptr;
                }
            }
        }

        // check for incorrect input
        assert_eq!(offs_stream, offs_stream_end);
        assert_eq!(len_stream, len_stream_end);

        // copy final literals
        if dst < dst_end {
            mode.CopyFinalLiterals(core, dst_end - dst, &mut dst, offset);
        } else {
            assert_eq!(dst, dst_end);
        }
    }
}

pub trait LeviathanMode {
    fn new(lzt: &LeviathanLzTable, dst_start: Pointer, core: &mut Core) -> Self;
    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: usize,
    );

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: usize,
    );
}

struct LeviathanModeSub {
    lit_stream: Pointer,
}

impl LeviathanMode for LeviathanModeSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Self {
        Self {
            lit_stream: lzt.lit_stream[0],
        }
    }
    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        last_offset: usize,
    ) {
        let mut litlen = (cmd >> 3) & 3;
        let next_len_stream = *len_stream + 1;
        if litlen == 3 {
            *len_stream = next_len_stream;
            litlen = (core.get_int(*len_stream) & 0xffffff) as usize;
        }
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, litlen);
        *dst += litlen;
        self.lit_stream += litlen;
    }

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: usize,
    ) {
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, final_len);
        *dst += final_len;
    }
}

struct LeviathanModeRaw {
    lit_stream: Pointer,
}

impl LeviathanMode for LeviathanModeRaw {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Self {
        Self {
            lit_stream: lzt.lit_stream[0],
        }
    }

    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        _: usize,
    ) {
        let mut litlen = (cmd >> 3) & 3;
        let next_len_stream = *len_stream + 1;
        if litlen == 3 {
            *len_stream = next_len_stream;
            litlen = (core.get_int(*len_stream) & 0xffffff) as usize;
        }
        core.repeat_copy_64(*dst, self.lit_stream, litlen);
        *dst += litlen;
        self.lit_stream += litlen;
    }

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        _: usize,
    ) {
        core.repeat_copy_64(*dst, self.lit_stream, final_len);
        *dst += final_len;
    }
}

struct LeviathanModeLamSub {
    lit_stream: Pointer,
    lam_lit_stream: Pointer,
}

impl LeviathanMode for LeviathanModeLamSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Self {
        Self {
            lit_stream: lzt.lit_stream[0],
            lam_lit_stream: lzt.lit_stream[1],
        }
    }

    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: usize,
    ) {
        let lit_cmd = cmd & 0x18;
        assert_ne!(lit_cmd, 0);

        let mut litlen = lit_cmd >> 3;
        let next_len_stream = *len_stream + 1;
        if litlen == 3 {
            *len_stream = next_len_stream;
            litlen = (core.get_int(*len_stream) & 0xffffff) as usize;
        }

        assert_ne!(litlen, 0, "lamsub mode requires one literal");
        litlen -= 1;

        let lam_byte = core.get_byte(self.lam_lit_stream) + core.get_byte(*dst + last_offset);
        core.set(*dst, lam_byte);
        self.lam_lit_stream += 1;
        *dst += 1;

        litlen = litlen.min(match_zone_end - *dst);
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, litlen);
        *dst += litlen;
        self.lit_stream += litlen;
    }

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        mut final_len: usize,
        dst: &mut Pointer,
        last_offset: usize,
    ) {
        let lam_byte = core.get_byte(self.lam_lit_stream) + core.get_byte(*dst + last_offset);
        core.set(*dst, lam_byte);
        self.lam_lit_stream += 1;
        *dst += 1;
        final_len -= 1;
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, final_len);
        *dst += final_len;
    }
}

struct LeviathanModeSubAnd<const NUM: usize> {
    lit_stream: [Pointer; NUM],
}

impl<const NUM: usize> LeviathanModeSubAnd<NUM> {
    const MASK: usize = NUM - 1;
    fn copy_literal(&mut self, core: &mut Core, dst: &mut Pointer, last_offset: usize) {
        let v = &mut self.lit_stream[dst.index & Self::MASK];
        core.set(*dst, core.get_byte(*v) + core.get_byte(*dst + last_offset));
        *v += 1;
        *dst += 1;
    }
}

impl<const NUM: usize> LeviathanMode for LeviathanModeSubAnd<NUM> {
    fn new(lzt: &LeviathanLzTable, dst_start: Pointer, _: &mut Core) -> Self {
        let base = !dst_start.index + 1;
        Self {
            lit_stream: core::array::from_fn(|i| lzt.lit_stream[(base + i) & Self::MASK]),
        }
    }

    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: usize,
    ) {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = core.get_int(*len_stream) as usize & 0xffffff;
            *len_stream += 1;
            assert!(litlen <= match_zone_end - *dst);
            for _ in 0..litlen {
                self.copy_literal(core, dst, last_offset);
            }
        } else if lit_cmd != 0 {
            self.copy_literal(core, dst, last_offset);
            if lit_cmd == 0x10 {
                self.copy_literal(core, dst, last_offset);
            }
        }
    }

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: usize,
    ) {
        for _ in 0..final_len {
            self.copy_literal(core, dst, last_offset);
        }
    }
}

struct LeviathanModeO1 {
    lit_streams: [Pointer; 16],
    next_lit: [u8; 16],
}

impl LeviathanMode for LeviathanModeO1 {
    fn new(lzt: &LeviathanLzTable, _: Pointer, core: &mut Core) -> Self {
        Self {
            lit_streams: core::array::from_fn(|i| lzt.lit_stream[i] + 1),
            next_lit: core::array::from_fn(|i| core.get_byte(lzt.lit_stream[i])),
        }
    }

    fn CopyLiterals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        _: usize,
    ) {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = core.get_int(*len_stream);
            *len_stream += 1;
            assert!(litlen > 0);
            let mut context = core.get_as_usize(*dst - 1);
            for _ in 0..litlen {
                self.copy_literal(core, dst, &mut context);
            }
        } else if lit_cmd != 0 {
            // either 1 or 2
            let mut context = core.get_as_usize(*dst - 1);
            self.copy_literal(core, dst, &mut context);
            if lit_cmd == 2 {
                self.copy_literal(core, dst, &mut context);
            }
        }
    }

    fn CopyFinalLiterals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        _: usize,
    ) {
        let mut context = core.get_as_usize(*dst - 1);
        for _ in 0..final_len {
            self.copy_literal(core, dst, &mut context);
        }
    }
}

impl LeviathanModeO1 {
    fn copy_literal(&mut self, core: &mut Core, dst: &mut Pointer, context: &mut usize) {
        let slot = *context >> 4;
        *context = self.next_lit[slot] as usize;
        core.set(*dst, *context as u8);
        *dst += 1;
        self.next_lit[slot] = core.get_byte(self.lit_streams[slot]);
        self.lit_streams[slot] += 1
    }
}