use crate::algorithm::Algorithm;
use crate::core::Core;
use crate::error::{ErrorContext, Res, WithContext};
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
impl ErrorContext for LeviathanLzTable {}

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
    ) -> Res<()> {
        let offset = (dst - dst_start)?;
        let mut lz = LeviathanLzTable::default();
        lz.read_lz_table(core, mode, src, src + src_used, dst, dst_size, offset)?;
        lz.process_lz_runs(core, mode, dst, dst_size, offset)
    }
}

impl LeviathanLzTable {
    fn read_lz_table(
        &mut self,
        core: &mut Core,
        chunk_type: usize,
        mut src: Pointer,
        src_end: Pointer,
        mut dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> Res<()> {
        let mut tmp = Pointer::tmp(0);
        let scratch = Pointer::scratch(0);
        let mut packed_offs_stream;
        let mut packed_len_stream;
        let mut out;
        let mut decode_count = 0;

        assert!(chunk_type <= 5, "invalid chunk type {}", chunk_type);
        assert!((src_end - src)? >= 13, "{}", (src_end - src)?);

        if offset == 0 {
            core.memmove(dst, src, 8);
            dst += 8;
            src += 8;
        }

        let mut offs_scaling = 0;
        let mut packed_offs_stream_extra = Pointer::null();
        let offs_stream_limit = dst_size / 3;

        if core.get_byte(src).at(self)? & 0x80 == 0 {
            // Decode packed offset stream, it's bounded by the command length.
            packed_offs_stream = tmp;
            src += core
                .decode_bytes(
                    &mut packed_offs_stream,
                    src,
                    src_end,
                    &mut self.offs_stream_size,
                    offs_stream_limit,
                    false,
                    scratch,
                )
                .at(self)?;
            tmp += self.offs_stream_size;
        } else {
            // uses the mode where distances are coded with 2 tables
            // and the transformation offs * scaling + low_bits
            offs_scaling = core.get_byte(src).at(self)? as i32 - 127;
            src += 1;

            packed_offs_stream = tmp;
            src += core
                .decode_bytes(
                    &mut packed_offs_stream,
                    src,
                    src_end,
                    &mut self.offs_stream_size,
                    offs_stream_limit,
                    false,
                    scratch,
                )
                .at(self)?;
            tmp += self.offs_stream_size;

            if offs_scaling != 1 {
                packed_offs_stream_extra = tmp;
                src += core
                    .decode_bytes(
                        &mut packed_offs_stream_extra,
                        src,
                        src_end,
                        &mut decode_count,
                        offs_stream_limit,
                        false,
                        scratch,
                    )
                    .at(self)?;
                self.assert_eq(decode_count, self.offs_stream_size)?;
                tmp += decode_count;
            }
        }

        // Decode packed litlen stream. It's bounded by 1/5 of dst_size.
        packed_len_stream = tmp;
        src += core
            .decode_bytes(
                &mut packed_len_stream,
                src,
                src_end,
                &mut self.len_stream_size,
                dst_size / 5,
                false,
                scratch,
            )
            .at(self)?;
        tmp += self.len_stream_size;

        // Reserve memory for final dist stream
        tmp = tmp.align(16);
        self.offs_stream = tmp.into();
        tmp += self.offs_stream_size * 4;

        // Reserve memory for final len stream
        tmp = tmp.align(16);
        self.len_stream = tmp.into();
        tmp += self.len_stream_size * 4;

        if chunk_type <= 1 {
            // Decode lit stream, bounded by dst_size
            out = tmp;
            src += core
                .decode_bytes(
                    &mut out,
                    src,
                    src_end,
                    &mut decode_count,
                    dst_size,
                    true,
                    scratch,
                )
                .at(self)?;
            self.lit_stream.push(out);
            self.lit_stream_size.push(decode_count);
        } else {
            let array_count = if chunk_type == 2 {
                2
            } else if chunk_type == 3 {
                4
            } else {
                16
            };
            src += core
                .decode_multi_array(
                    src,
                    src_end,
                    tmp,
                    Pointer::tmp(usize::MAX),
                    &mut self.lit_stream,
                    &mut self.lit_stream_size,
                    array_count,
                    &mut decode_count,
                    true,
                    scratch,
                )
                .at(self)?;
        }
        tmp += decode_count;
        self.lit_stream_total = decode_count;

        assert!(src < src_end);

        let flag = core.get_byte(src).at(self)?;
        if (flag & 0x80) == 0 {
            // Decode command stream, bounded by dst_size
            out = tmp;
            src += core
                .decode_bytes(
                    &mut out,
                    src,
                    src_end,
                    &mut decode_count,
                    dst_size,
                    true,
                    scratch,
                )
                .at(self)?;
            self.cmd_stream = out;
            self.cmd_stream_size = decode_count;
            tmp += decode_count;
        } else {
            self.assert_eq(flag, 0x83)?;
            src += 1;
            src += core
                .decode_multi_array(
                    src,
                    src_end,
                    tmp,
                    Pointer::tmp(usize::MAX),
                    &mut self.multi_cmd_ptr,
                    &mut self.multi_cmd_end,
                    8,
                    &mut decode_count,
                    true,
                    scratch,
                )
                .at(self)?;

            self.cmd_stream = Pointer::null();
            self.cmd_stream_size = decode_count;
            tmp += decode_count;
        }

        core.unpack_offsets(
            src,
            src_end,
            packed_offs_stream,
            packed_offs_stream_extra,
            self.offs_stream_size,
            offs_scaling,
            packed_len_stream,
            self.len_stream_size,
            self.offs_stream,
            self.len_stream,
            false,
        )
        .at(self)?;

        Ok(())
    }
    pub fn process_lz_runs(
        &mut self,
        core: &mut Core,
        mode: usize,
        dst: Pointer,
        dst_size: usize,
        offset: usize,
    ) -> Res<()> {
        let dst_cur = if offset == 0 { dst + 8 } else { dst };
        let dst_end = dst + dst_size;
        let dst_start = (dst - offset)?;
        match mode {
            0 => self.process_lz::<LeviathanModeSub>(core, dst_cur, dst, dst_end, dst_start),
            1 => self.process_lz::<LeviathanModeRaw>(core, dst_cur, dst, dst_end, dst_start),
            2 => self.process_lz::<LeviathanModeLamSub>(core, dst_cur, dst, dst_end, dst_start),
            3 => self.process_lz::<LeviathanModeSubAnd<4>>(core, dst_cur, dst, dst_end, dst_start),
            4 => self.process_lz::<LeviathanModeO1>(core, dst_cur, dst, dst_end, dst_start),
            5 => self.process_lz::<LeviathanModeSubAnd<16>>(core, dst_cur, dst, dst_end, dst_start),
            _ => self.raise(format!("Invalid mode: {}", mode))?,
        }
    }
    pub fn process_lz<Mode: LeviathanMode>(
        &mut self,
        core: &mut Core,
        mut dst: Pointer,
        dst_start: Pointer,
        dst_end: Pointer,
        window_base: Pointer,
    ) -> Res<()> {
        let multi_cmd = self.cmd_stream.is_null();
        let mut cmd_stream = self.cmd_stream;
        let cmd_stream_end = cmd_stream + self.cmd_stream_size;
        let mut len_stream = self.len_stream;
        let mut len_stream_end = len_stream + self.len_stream_size;

        let mut offs_stream = self.offs_stream;
        let offs_stream_end = offs_stream + self.offs_stream_size;
        let mut copyfrom;
        let match_zone_end = if (dst_end - dst_start)? >= 16 {
            (dst_end - 16)?
        } else {
            dst_start
        };

        let mut recent_offs: [i32; 16] = core::array::from_fn(|i| match i {
            8..=14 => -8,
            _ => 0,
        });

        let mut offset = -8i32;

        let mut mode = Mode::new(self, dst_start, core).at(self)?;

        let mut cmd_stream_left = 0;
        let mut multi_cmd_stream = [Pointer::null(); 8];
        let mut cmd_stream_ptr = &mut multi_cmd_stream[0];
        if multi_cmd {
            for (i, p) in multi_cmd_stream.iter_mut().enumerate() {
                *p = self.multi_cmd_ptr[i.wrapping_sub(dst_start.index) & 7];
            }
            cmd_stream_left = self.cmd_stream_size;
            cmd_stream_ptr = &mut multi_cmd_stream[dst.index & 7];
            cmd_stream = *cmd_stream_ptr;
        }

        loop {
            let cmd;

            if !multi_cmd {
                if cmd_stream >= cmd_stream_end {
                    break;
                }
                cmd = core.get_byte(cmd_stream).at(self)? as usize;
                cmd_stream += 1;
            } else {
                if cmd_stream_left == 0 {
                    break;
                }
                cmd_stream_left -= 1;
                cmd = core.get_byte(cmd_stream).at(self)? as usize;
                *cmd_stream_ptr = cmd_stream + 1;
            }

            let offs_index = cmd >> 5;
            assert!(offs_index < 8);
            let mut matchlen = (cmd & 7) + 2;

            recent_offs[15] = core.get_int(offs_stream).at(core)?;

            mode.copy_literals(core, cmd, &mut dst, &mut len_stream, match_zone_end, offset)
                .at(self)?;

            offset = recent_offs[offs_index + 8];

            // Permute the recent offsets table
            let mut temp = [0; 4];

            temp.copy_from_slice(&recent_offs[offs_index + 4..][..4]);
            recent_offs.copy_within(offs_index..offs_index + 4, offs_index + 1);
            recent_offs[offs_index + 5..][..4].copy_from_slice(&temp);
            recent_offs[8] = offset;
            if offs_index == 7 {
                offs_stream += 1;
            }

            copyfrom = dst + offset;
            assert!(
                copyfrom >= window_base,
                "offset out of bounds {} {}",
                offset,
                (dst - window_base)?
            );

            if matchlen == 9 {
                assert!(len_stream < len_stream_end, "len stream empty");
                len_stream_end = (len_stream_end - 1)?;
                matchlen = (core.get_int(len_stream_end).at(core)? + 6) as usize;
                assert!(matchlen <= (dst_end - dst)? - 8, "no space in buf");
                core.repeat_copy_64(dst, copyfrom, matchlen);
                dst += matchlen;
                if multi_cmd {
                    cmd_stream_ptr = &mut multi_cmd_stream[dst.index & 7];
                    cmd_stream = *cmd_stream_ptr;
                }
            } else {
                core.repeat_copy_64(dst, copyfrom, matchlen);
                dst += matchlen;
                if multi_cmd {
                    cmd_stream_ptr = &mut multi_cmd_stream[dst.index & 7];
                    cmd_stream = *cmd_stream_ptr;
                }
            }
        }

        // check for incorrect input
        self.assert_eq(offs_stream, offs_stream_end)?;
        self.assert_eq(len_stream, len_stream_end)?;

        // copy final literals
        if dst < dst_end {
            mode.copy_final_literals(core, (dst_end - dst)?, &mut dst, offset)
                .at(self)?;
        } else {
            self.assert_eq(dst, dst_end)?;
        }
        Ok(())
    }
}

pub trait LeviathanMode: Sized {
    fn new(lzt: &LeviathanLzTable, dst_start: Pointer, core: &mut Core) -> Res<Self>;
    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: i32,
    ) -> Res<()>;

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: i32,
    ) -> Res<()>;
}

struct LeviathanModeSub {
    lit_stream: Pointer,
}

impl ErrorContext for LeviathanModeSub {}

impl LeviathanMode for LeviathanModeSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0],
        })
    }
    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        last_offset: i32,
    ) -> Res<()> {
        let mut litlen = (cmd >> 3) & 3;
        if litlen == 3 {
            litlen = (core.get_int(*len_stream).at(core)? & 0xffffff) as usize;
            *len_stream += 1;
        }
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, litlen)
            .at(self)?;
        *dst += litlen;
        self.lit_stream += litlen;
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: i32,
    ) -> Res<()> {
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, final_len)
            .at(self)?;
        *dst += final_len;
        Ok(())
    }
}

struct LeviathanModeRaw {
    lit_stream: Pointer,
}

impl LeviathanMode for LeviathanModeRaw {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0],
        })
    }

    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        _: i32,
    ) -> Res<()> {
        let mut litlen = (cmd >> 3) & 3;
        if litlen == 3 {
            litlen = (core.get_int(*len_stream).at(core)? & 0xffffff) as usize;
            *len_stream += 1;
        }
        core.repeat_copy_64(*dst, self.lit_stream, litlen);
        *dst += litlen;
        self.lit_stream += litlen;
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        _: i32,
    ) -> Res<()> {
        core.repeat_copy_64(*dst, self.lit_stream, final_len);
        *dst += final_len;
        Ok(())
    }
}

struct LeviathanModeLamSub {
    lit_stream: Pointer,
    lam_lit_stream: Pointer,
}

impl ErrorContext for LeviathanModeLamSub {}

impl LeviathanMode for LeviathanModeLamSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0],
            lam_lit_stream: lzt.lit_stream[1],
        })
    }

    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0 {
            return Ok(());
        }

        let mut litlen = lit_cmd >> 3;
        if litlen == 3 {
            litlen = (core.get_int(*len_stream).at(core)? & 0xffffff) as usize;
            *len_stream += 1;
        }

        assert_ne!(litlen, 0, "lamsub mode requires one literal");
        assert!(litlen < (match_zone_end - *dst)?, "out of bounds");
        litlen -= 1;

        let lam_byte = core
            .get_byte(self.lam_lit_stream)?
            .wrapping_add(core.get_byte(*dst + last_offset).at(self)?);
        core.set(*dst, lam_byte);
        self.lam_lit_stream += 1;
        *dst += 1;

        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, litlen)
            .at(self)?;
        *dst += litlen;
        self.lit_stream += litlen;
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        mut final_len: usize,
        dst: &mut Pointer,
        last_offset: i32,
    ) -> Res<()> {
        let lam_byte = core
            .get_byte(self.lam_lit_stream)?
            .wrapping_add(core.get_byte(*dst + last_offset).at(self)?);
        core.set(*dst, lam_byte);
        self.lam_lit_stream += 1;
        *dst += 1;
        final_len -= 1;
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, final_len)
            .at(self)?;
        *dst += final_len;
        Ok(())
    }
}

struct LeviathanModeSubAnd<const NUM: usize> {
    lit_stream: [Pointer; NUM],
}

impl<const T: usize> ErrorContext for LeviathanModeSubAnd<T> {}

impl<const NUM: usize> LeviathanModeSubAnd<NUM> {
    const MASK: usize = NUM - 1;
    fn copy_literal(&mut self, core: &mut Core, dst: &mut Pointer, last_offset: i32) -> Res<()> {
        let v = &mut self.lit_stream[dst.index & Self::MASK];
        core.set(
            *dst,
            core.get_byte(*v)?
                .wrapping_add(core.get_byte(*dst + last_offset)?),
        );
        *v += 1;
        *dst += 1;
        Ok(())
    }
}

impl<const NUM: usize> LeviathanMode for LeviathanModeSubAnd<NUM> {
    fn new(lzt: &LeviathanLzTable, dst_start: Pointer, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: core::array::from_fn(|i| {
                lzt.lit_stream[i.wrapping_sub(dst_start.index) & Self::MASK]
            }),
        })
    }

    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        match_zone_end: Pointer,
        last_offset: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = core.get_int(*len_stream).at(core)? as usize & 0xffffff;
            *len_stream += 1;
            assert!(litlen <= (match_zone_end - *dst)?);
            for _ in 0..litlen {
                self.copy_literal(core, dst, last_offset).at(self)?;
            }
        } else if lit_cmd != 0 {
            self.copy_literal(core, dst, last_offset).at(self)?;
            if lit_cmd == 0x10 {
                self.copy_literal(core, dst, last_offset).at(self)?;
            }
        }
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        last_offset: i32,
    ) -> Res<()> {
        for _ in 0..final_len {
            self.copy_literal(core, dst, last_offset).at(self)?;
        }
        Ok(())
    }
}

struct LeviathanModeO1 {
    lit_streams: [Pointer; 16],
    next_lit: [u8; 16],
    context: u8,
}

impl ErrorContext for LeviathanModeO1 {}

impl LeviathanMode for LeviathanModeO1 {
    fn new(lzt: &LeviathanLzTable, _: Pointer, core: &mut Core) -> Res<Self> {
        let mut result = Self {
            lit_streams: core::array::from_fn(|i| lzt.lit_stream[i] + 1),
            next_lit: [0; 16],
            context: 0,
        };
        for (i, v) in result.next_lit.iter_mut().enumerate() {
            *v = core.get_byte(lzt.lit_stream[i])?
        }
        Ok(result)
    }

    fn copy_literals(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer,
        len_stream: &mut IntPointer,
        _: Pointer,
        _: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = core.get_int(*len_stream).at(core)?;
            *len_stream += 1;
            assert!(litlen > 0);
            self.context = core.get_byte((*dst - 1)?).at(self)?;
            for _ in 0..litlen {
                self.copy_literal(core, dst).at(self)?;
            }
        } else if lit_cmd != 0 {
            // either 1 or 2
            self.context = core.get_byte((*dst - 1)?).at(self)?;
            self.copy_literal(core, dst).at(self)?;
            if lit_cmd == 0x10 {
                self.copy_literal(core, dst).at(self)?;
            }
        }
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer,
        _: i32,
    ) -> Res<()> {
        self.context = core.get_byte((*dst - 1)?).at(self)?;
        for _ in 0..final_len {
            self.copy_literal(core, dst).at(self)?;
        }
        Ok(())
    }
}

impl LeviathanModeO1 {
    fn copy_literal(&mut self, core: &mut Core, dst: &mut Pointer) -> Res<()> {
        let slot = (self.context >> 4) as usize;
        self.context = self.next_lit[slot];
        core.set(*dst, self.context);
        *dst += 1;
        self.next_lit[slot] = core.get_byte(self.lit_streams[slot]).at(self)?;
        self.lit_streams[slot] += 1;
        Ok(())
    }
}
