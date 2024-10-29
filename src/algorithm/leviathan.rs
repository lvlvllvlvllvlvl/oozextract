use crate::algorithm::Algorithm;
use crate::decoder::pointer::{Pointer, PointerDest};
use crate::decoder::{pointer, Core};
use crate::ooz::error::{ErrorContext, Res, ResultBuilder, SliceErrors, WithContext};

#[derive(Default)]
pub struct LeviathanLzTable {
    offs_stream: Vec<i32>,
    len_stream: Vec<i32>,
    lit_stream: [(Pointer<{ PointerDest::TEMP }>, usize); 16],
    lit_stream_total: usize,
    multi_cmd_ptr: [(Pointer<{ PointerDest::TEMP }>, usize); 8],
    cmd_stream: Option<Pointer<{ PointerDest::TEMP }>>,
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
        src: Pointer<{ PointerDest::INPUT }>,
        src_used: usize,
        dst_start: Pointer<{ PointerDest::OUTPUT }>,
        dst: Pointer<{ PointerDest::OUTPUT }>,
        dst_size: usize,
    ) -> Res<()> {
        let offset = dst - dst_start;
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
        mut src: Pointer<{ PointerDest::INPUT }>,
        src_end: Pointer<{ PointerDest::INPUT }>,
        mut dst: Pointer<{ PointerDest::OUTPUT }>,
        dst_size: usize,
        offset: usize,
    ) -> Res<()> {
        let mut tmp = pointer::tmp(0);
        let scratch = pointer::scratch(0);
        let mut packed_offs_stream;
        let mut packed_len_stream;
        let mut offs_stream_size = 0;
        let mut len_stream_size = 0;
        let mut out;
        let mut decode_count = 0;

        self.assert_le(chunk_type, 5)?;
        self.assert_le(13, src_end - src)?;

        if offset == 0 {
            core.copy_bytes(dst, src, 8).at(self)?;
            dst += 8;
            src += 8;
        }

        let mut offs_scaling = 0;
        let mut packed_offs_stream_extra = None;
        let offs_stream_limit = dst_size / 3;

        if core.get_byte(src).at(self)? & 0x80 == 0 {
            // Decode packed offset stream, it's bounded by the command length.
            packed_offs_stream = tmp;
            src += core
                .decode_bytes(
                    &mut packed_offs_stream,
                    src,
                    src_end,
                    &mut offs_stream_size,
                    offs_stream_limit,
                    false,
                    scratch,
                )
                .at(self)?;
            tmp += offs_stream_size;
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
                    &mut offs_stream_size,
                    offs_stream_limit,
                    false,
                    scratch,
                )
                .at(self)?;
            tmp += offs_stream_size;

            if offs_scaling != 1 {
                packed_offs_stream_extra = Some(tmp);
                src += core
                    .decode_bytes(
                        packed_offs_stream_extra
                            .as_mut()
                            .expect("value assigned above"),
                        src,
                        src_end,
                        &mut decode_count,
                        offs_stream_limit,
                        false,
                        scratch,
                    )
                    .at(self)?;
                self.assert_eq(decode_count, offs_stream_size)?;
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
                &mut len_stream_size,
                dst_size / 5,
                false,
                scratch,
            )
            .at(self)?;
        tmp += len_stream_size;

        self.offs_stream = vec![0; offs_stream_size];
        self.len_stream = vec![0; len_stream_size];

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
            self.lit_stream[0] = (out, decode_count);
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
                    pointer::tmp(usize::MAX),
                    &mut self.lit_stream[..array_count],
                    &mut decode_count,
                    true,
                    scratch,
                )
                .at(self)?;
        }
        tmp += decode_count;
        self.lit_stream_total = decode_count;

        self.assert_lt(src, src_end)?;

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
            self.cmd_stream = Some(out);
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
                    pointer::tmp(usize::MAX),
                    &mut self.multi_cmd_ptr,
                    &mut decode_count,
                    true,
                    scratch,
                )
                .at(self)?;

            self.cmd_stream = None;
            self.cmd_stream_size = decode_count;
            tmp += decode_count;
        }

        if let Some(extra) = packed_offs_stream_extra {
            core.unpack_offsets(
                src,
                src_end,
                packed_offs_stream,
                extra,
                offs_scaling,
                packed_len_stream,
                self.offs_stream.as_mut(),
                self.len_stream.as_mut(),
                false,
            )
            .at(self)?;
        } else {
            core.unpack_offsets(
                src,
                src_end,
                packed_offs_stream,
                pointer::null(),
                offs_scaling,
                packed_len_stream,
                self.offs_stream.as_mut(),
                self.len_stream.as_mut(),
                false,
            )
            .at(self)?;
        }

        Ok(())
    }
    pub fn process_lz_runs(
        &mut self,
        core: &mut Core,
        mode: usize,
        dst: Pointer<{ PointerDest::OUTPUT }>,
        dst_size: usize,
        offset: usize,
    ) -> Res<()> {
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
            _ => self.raise(format!("Invalid mode: {}", mode))?,
        }
    }
    pub fn process_lz<Mode: LeviathanMode>(
        &mut self,
        core: &mut Core,
        mut dst: Pointer<{ PointerDest::OUTPUT }>,
        dst_start: Pointer<{ PointerDest::OUTPUT }>,
        dst_end: Pointer<{ PointerDest::OUTPUT }>,
        window_base: Pointer<{ PointerDest::OUTPUT }>,
    ) -> Res<()> {
        let mut len_stream = self.len_stream.iter().copied();
        let mut offs_stream = self.offs_stream.iter().copied().peekable();
        let mut copyfrom;
        let match_zone_end = if (dst_end - dst_start) >= 16 {
            dst_end - 16
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
        let mut multi_cmd_stream: Option<[Pointer<{ PointerDest::TEMP }>; 8]> = None;
        let mut cmd_stream_ptr = None;
        let mut cmd_stream = if let Some(cmd_stream) = self.cmd_stream {
            cmd_stream
        } else {
            cmd_stream_left = self.cmd_stream_size;
            let streams =
                core::array::from_fn(|i| self.multi_cmd_ptr[i.wrapping_sub(dst_start.index) & 7].0);
            let cmd_stream = streams.get_copy(dst.index & 7)?;
            multi_cmd_stream = Some(streams);
            cmd_stream_ptr = multi_cmd_stream
                .as_mut()
                .map(|streams| &mut streams[dst.index & 7]);
            cmd_stream
        };
        let cmd_stream_end = cmd_stream + self.cmd_stream_size;

        loop {
            let cmd;

            if let Some(cmd_stream_ptr) = cmd_stream_ptr.as_mut() {
                if cmd_stream_left == 0 {
                    break;
                }
                cmd_stream_left -= 1;
                cmd = core.get_byte(cmd_stream).at(self)? as usize;
                **cmd_stream_ptr = cmd_stream + 1;
            } else {
                if cmd_stream >= cmd_stream_end {
                    break;
                }
                cmd = core.get_byte(cmd_stream).at(self)? as usize;
                cmd_stream += 1;
            }

            let offs_index = cmd >> 5;
            self.assert_le(offs_index, 8)?;
            let mut matchlen = (cmd & 7) + 2;

            recent_offs[15] = offs_stream.peek().copied().unwrap_or_default();

            mode.copy_literals(core, cmd, &mut dst, &mut len_stream, match_zone_end, offset)
                .at(self)?;

            offset = recent_offs.get_copy(offs_index + 8)?;

            // Permute the recent offsets table
            recent_offs.copy_within(offs_index..offs_index + 8, offs_index + 1);
            recent_offs[8] = offset;
            if offs_index == 7 {
                offs_stream.next();
            }

            copyfrom = dst + offset;
            self.assert_le(window_base, copyfrom)?;

            if matchlen == 9 {
                matchlen = len_stream.next_back().msg_of(&"len stream empty")? as usize + 6;
                self.assert_le(matchlen, (dst_end - dst) - 8)?;
                core.repeat_copy_64(dst, copyfrom, matchlen).at(self)?;
                dst += matchlen;
            } else {
                core.repeat_copy_64(dst, copyfrom, matchlen).at(self)?;
                dst += matchlen;
            }

            cmd_stream_ptr = if let Some(multi_cmd_stream) = multi_cmd_stream.as_mut() {
                cmd_stream = multi_cmd_stream[dst.index & 7];
                Some(&mut multi_cmd_stream[dst.index & 7])
            } else {
                None
            };
        }

        // check for incorrect input
        self.assert_eq(offs_stream.len(), 0)?;
        self.assert(len_stream.next().is_none(), "len stream not finished")?;

        // copy final literals
        if dst < dst_end {
            mode.copy_final_literals(core, dst_end - dst, &mut dst, offset)
                .at(self)?;
        } else {
            self.assert_eq(dst, dst_end)?;
        }
        Ok(())
    }
}

pub trait LeviathanMode: Sized {
    fn new(
        lzt: &LeviathanLzTable,
        dst_start: Pointer<{ PointerDest::OUTPUT }>,
        core: &mut Core,
    ) -> Res<Self>;
    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        match_zone_end: Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()>;

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()>;
}

struct LeviathanModeSub {
    lit_stream: Pointer<{ PointerDest::TEMP }>,
}

impl ErrorContext for LeviathanModeSub {}

impl LeviathanMode for LeviathanModeSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer<{ PointerDest::OUTPUT }>, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0].0,
        })
    }
    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        _: Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        let mut litlen = (cmd >> 3) & 3;
        if litlen == 3 {
            litlen = (len_stream.next().err()? & 0xffffff) as usize;
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
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        core.copy_64_add(*dst, self.lit_stream, *dst + last_offset, final_len)
            .at(self)?;
        *dst += final_len;
        Ok(())
    }
}

struct LeviathanModeRaw {
    lit_stream: Pointer<{ PointerDest::TEMP }>,
}

impl ErrorContext for LeviathanModeRaw {}

impl LeviathanMode for LeviathanModeRaw {
    fn new(lzt: &LeviathanLzTable, _: Pointer<{ PointerDest::OUTPUT }>, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0].0,
        })
    }

    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        _: Pointer<{ PointerDest::OUTPUT }>,
        _: i32,
    ) -> Res<()> {
        let mut litlen = (cmd >> 3) & 3;
        if litlen == 3 {
            litlen = (len_stream.next().err()? & 0xffffff) as usize;
        }
        core.repeat_copy_64(*dst, self.lit_stream, litlen)
            .at(self)?;
        *dst += litlen;
        self.lit_stream += litlen;
        Ok(())
    }

    fn copy_final_literals(
        &mut self,
        core: &mut Core,
        final_len: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        _: i32,
    ) -> Res<()> {
        core.repeat_copy_64(*dst, self.lit_stream, final_len)
            .at(self)?;
        *dst += final_len;
        Ok(())
    }
}

struct LeviathanModeLamSub {
    lit_stream: Pointer<{ PointerDest::TEMP }>,
    lam_lit_stream: Pointer<{ PointerDest::TEMP }>,
}

impl ErrorContext for LeviathanModeLamSub {}

impl LeviathanMode for LeviathanModeLamSub {
    fn new(lzt: &LeviathanLzTable, _: Pointer<{ PointerDest::OUTPUT }>, _: &mut Core) -> Res<Self> {
        Ok(Self {
            lit_stream: lzt.lit_stream[0].0,
            lam_lit_stream: lzt.lit_stream[1].0,
        })
    }

    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        match_zone_end: Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0 {
            return Ok(());
        }

        let mut litlen = lit_cmd >> 3;
        if litlen == 3 {
            litlen = (len_stream.next().err()? & 0xffffff) as usize;
        }

        self.assert_ne(litlen, 0)?;
        self.assert_lt(litlen, match_zone_end - *dst)?;
        litlen -= 1;

        let lam_byte = core
            .get_byte(self.lam_lit_stream)?
            .wrapping_add(core.get_byte(*dst + last_offset).at(self)?);
        core.set(*dst, lam_byte).at(self)?;
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
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        let lam_byte = core
            .get_byte(self.lam_lit_stream)?
            .wrapping_add(core.get_byte(*dst + last_offset).at(self)?);
        core.set(*dst, lam_byte).at(self)?;
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
    lit_stream: [Pointer<{ PointerDest::TEMP }>; NUM],
}

impl<const T: usize> ErrorContext for LeviathanModeSubAnd<T> {}

impl<const NUM: usize> LeviathanModeSubAnd<NUM> {
    const MASK: usize = NUM - 1;
    fn copy_literal(
        &mut self,
        core: &mut Core,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        assert!(Self::MASK < 16);
        let v = &mut self.lit_stream[dst.index & Self::MASK];
        core.set(
            *dst,
            core.get_byte(*v)?
                .wrapping_add(core.get_byte(*dst + last_offset)?),
        )?;
        *v += 1;
        *dst += 1;
        Ok(())
    }
}

impl<const NUM: usize> LeviathanMode for LeviathanModeSubAnd<NUM> {
    fn new(
        lzt: &LeviathanLzTable,
        dst_start: Pointer<{ PointerDest::OUTPUT }>,
        _: &mut Core,
    ) -> Res<Self> {
        Ok(Self {
            lit_stream: core::array::from_fn(|i| {
                lzt.lit_stream[i.wrapping_sub(dst_start.index) & Self::MASK].0
            }),
        })
    }

    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        match_zone_end: Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = len_stream.next().err()? as usize & 0xffffff;
            self.assert_le(litlen, match_zone_end - *dst)?;
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
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        last_offset: i32,
    ) -> Res<()> {
        for _ in 0..final_len {
            self.copy_literal(core, dst, last_offset).at(self)?;
        }
        Ok(())
    }
}

struct LeviathanModeO1 {
    lit_streams: [Pointer<{ PointerDest::TEMP }>; 16],
    next_lit: [u8; 16],
    context: u8,
}

impl ErrorContext for LeviathanModeO1 {}

impl LeviathanMode for LeviathanModeO1 {
    #[allow(clippy::indexing_slicing)]
    fn new(
        lzt: &LeviathanLzTable,
        _: Pointer<{ PointerDest::OUTPUT }>,
        core: &mut Core,
    ) -> Res<Self> {
        let mut result = Self {
            lit_streams: lzt.lit_stream.map(|(p, _)| p + 1),
            next_lit: [0; 16],
            context: 0,
        };
        for (i, v) in result.next_lit.iter_mut().enumerate() {
            *v = core.get_byte(lzt.lit_stream.get(i).map(|&(p, _)| p).unwrap_or_default())?
        }
        Ok(result)
    }

    fn copy_literals<Iter: Iterator<Item = i32>>(
        &mut self,
        core: &mut Core,
        cmd: usize,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        len_stream: &mut Iter,
        _: Pointer<{ PointerDest::OUTPUT }>,
        _: i32,
    ) -> Res<()> {
        let lit_cmd = cmd & 0x18;
        if lit_cmd == 0x18 {
            let litlen = len_stream.next().err()?;
            self.assert_lt(0, litlen)?;
            self.context = core.get_byte(*dst - 1).at(self)?;
            for _ in 0..litlen {
                self.copy_literal(core, dst).at(self)?;
            }
        } else if lit_cmd != 0 {
            // either 1 or 2
            self.context = core.get_byte(*dst - 1).at(self)?;
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
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
        _: i32,
    ) -> Res<()> {
        self.context = core.get_byte(*dst - 1).at(self)?;
        for _ in 0..final_len {
            self.copy_literal(core, dst).at(self)?;
        }
        Ok(())
    }
}

impl LeviathanModeO1 {
    #[allow(clippy::indexing_slicing)] // u8 >> 4 can safely index [_; 16]
    fn copy_literal(
        &mut self,
        core: &mut Core,
        dst: &mut Pointer<{ PointerDest::OUTPUT }>,
    ) -> Res<()> {
        let slot = (self.context >> 4) as usize;
        self.context = self.next_lit[slot];
        core.set(*dst, self.context).at(self)?;
        *dst += 1;
        self.next_lit[slot] = core.get_byte(self.lit_streams[slot]).at(self)?;
        self.lit_streams[slot] += 1;
        Ok(())
    }
}
