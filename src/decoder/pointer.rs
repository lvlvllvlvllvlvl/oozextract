use crate::decoder::Core;
use crate::ooz::error::{ErrorBuilder, ErrorContext, Res, ResultBuilder};
use std::fmt::{Display, Formatter};
use std::mem::size_of;
use wide::{u64x2, u8x16};

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum PointerDest {
    #[default]
    Null = 0,
    Input = 1,
    Output = 2,
    Scratch = 3,
    Temp = 4,
}

impl PointerDest {
    const fn from(value: u8) -> Self {
        match value {
            v if v == PointerDest::Input as u8 => PointerDest::Input,
            v if v == PointerDest::Output as u8 => PointerDest::Output,
            v if v == PointerDest::Scratch as u8 => PointerDest::Scratch,
            v if v == PointerDest::Temp as u8 => PointerDest::Temp,
            _ => PointerDest::Null,
        }
    }
    pub const NULL: u8 = PointerDest::Null as u8;
    pub const INPUT: u8 = PointerDest::Input as u8;
    pub const OUTPUT: u8 = PointerDest::Output as u8;
    pub const SCRATCH: u8 = PointerDest::Scratch as u8;
    pub const TEMP: u8 = PointerDest::Temp as u8;
}

impl Display for PointerDest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
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
pub(crate) struct Pointer<const DEST: u8> {
    pub index: usize,
}

impl<const DEST: u8> Display for Pointer<DEST> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}[{}]", Self::INTO, self.index)
    }
}

impl<const DEST: u8> ErrorContext for Pointer<DEST> {}

pub fn input(index: usize) -> Pointer<{ PointerDest::INPUT }> {
    Pointer { index }
}
pub fn output(index: usize) -> Pointer<{ PointerDest::OUTPUT }> {
    Pointer { index }
}
pub fn scratch(index: usize) -> Pointer<{ PointerDest::SCRATCH }> {
    Pointer { index }
}
pub fn tmp(index: usize) -> Pointer<{ PointerDest::TEMP }> {
    Pointer { index }
}
pub fn null() -> Pointer<{ PointerDest::NULL }> {
    Default::default()
}

impl<const DEST: u8> Pointer<DEST> {
    const INTO: PointerDest = PointerDest::from(DEST);

    pub const fn dest(&self) -> PointerDest {
        Self::INTO
    }
    pub const fn is_null(&self) -> bool {
        DEST == PointerDest::Null as u8
    }
    pub fn debug(&self, _: usize) {
        // do nothing (there are no bugs)
    }
}

impl<const DEST: u8> std::ops::Add<usize> for Pointer<DEST> {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index + rhs,
            ..self
        }
    }
}

impl<const DEST: u8> std::ops::Add<usize> for &Pointer<DEST> {
    type Output = Pointer<DEST>;

    fn add(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index + rhs,
            ..*self
        }
    }
}

impl<const DEST: u8> std::ops::Add<i32> for Pointer<DEST> {
    type Output = Self;

    fn add(self, rhs: i32) -> Self::Output {
        Pointer {
            index: self.index.wrapping_add_signed(rhs as _),
            ..self
        }
    }
}

impl<const DEST: u8> std::ops::AddAssign<usize> for Pointer<DEST> {
    fn add_assign(&mut self, rhs: usize) {
        self.index += rhs
    }
}

impl<const DEST: u8> std::ops::SubAssign<usize> for Pointer<DEST> {
    fn sub_assign(&mut self, rhs: usize) {
        self.index -= rhs
    }
}

impl<const DEST: u8> std::ops::AddAssign<i32> for Pointer<DEST> {
    fn add_assign(&mut self, rhs: i32) {
        self.index = self.index.wrapping_add_signed(rhs as _)
    }
}

impl<const DEST: u8> std::ops::SubAssign<i32> for Pointer<DEST> {
    fn sub_assign(&mut self, rhs: i32) {
        self.index = self.index.wrapping_add_signed(-rhs as _)
    }
}

impl<const DEST: u8> std::ops::Sub<Pointer<DEST>> for Pointer<DEST> {
    type Output = usize;

    fn sub(self, rhs: Pointer<DEST>) -> Self::Output {
        self.index.wrapping_sub(rhs.index)
    }
}

impl<const DEST: u8> std::ops::Sub<usize> for Pointer<DEST> {
    type Output = Self;

    fn sub(self, rhs: usize) -> Self::Output {
        Pointer {
            index: self.index.wrapping_sub(rhs),
            ..self
        }
    }
}

impl<const DEST: u8> std::ops::Sub<u32> for Pointer<DEST> {
    type Output = Self;

    fn sub(self, rhs: u32) -> Self::Output {
        self.sub(rhs as usize)
    }
}

impl<const DEST: u8> std::ops::Sub<i32> for Pointer<DEST> {
    type Output = Self;

    fn sub(self, rhs: i32) -> Self::Output {
        Self {
            index: self.index.wrapping_add_signed(-rhs as _),
            ..self
        }
    }
}

impl Core<'_> {
    pub fn get_byte<const DEST: u8>(&self, p: Pointer<DEST>) -> Res<u8> {
        Ok(match p.dest() {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input.get(p.index),
            PointerDest::Output => self.output.get(p.index),
            PointerDest::Scratch => self.scratch.get(p.index),
            PointerDest::Temp => self.tmp.get(p.index),
        }
        .copied()
        .msg_of(&p)?)
    }
    pub fn get_slice<const DEST: u8>(&mut self, p: Pointer<DEST>, n: usize) -> Res<&[u8]> {
        Ok(match p.dest() {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input.get(p.index..p.index + n),
            PointerDest::Output => self.output.get(p.index..p.index + n),
            PointerDest::Scratch => self.scratch.get(p.index..p.index + n),
            PointerDest::Temp => self.tmp.get(p.index..p.index + n),
        }
        .message(|_| format!("oob {}..{}", p, p.index + n))?)
    }
    pub fn get_le_bytes<const DEST: u8>(&mut self, p: Pointer<DEST>, n: usize) -> Res<usize> {
        let mut bytes = [0; size_of::<usize>()];
        bytes[..n].copy_from_slice(self.get_slice(p, n)?);
        Ok(usize::from_le_bytes(bytes))
    }
    pub fn get_be_bytes<const DEST: u8>(&mut self, p: Pointer<DEST>, n: usize) -> Res<usize> {
        const B: usize = size_of::<usize>();
        let mut bytes = [0; B];
        bytes[B - n..].copy_from_slice(self.get_slice(p, n)?);
        Ok(usize::from_be_bytes(bytes))
    }
    pub fn get_arr<const DEST: u8, const LEN: usize>(
        &mut self,
        p: Pointer<DEST>,
    ) -> Res<[u8; LEN]> {
        let src = match p.dest() {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input,
            PointerDest::Output => self.output,
            PointerDest::Scratch => self.scratch,
            PointerDest::Temp => self.tmp,
        };
        let len = LEN.min(src.len() - p.index);
        let slice = &src[p.index..p.index + len];
        if len == LEN {
            Ok(slice.try_into().expect("len == LEN"))
        } else {
            Ok(core::array::from_fn(|i| {
                slice.get(i).copied().unwrap_or_default()
            }))
        }
    }
    pub fn set_arr<const DEST: u8, const LEN: usize>(
        &mut self,
        p: Pointer<DEST>,
        v: [u8; LEN],
    ) -> Res<()> {
        let src = match p.dest() {
            PointerDest::Null => panic!(),
            PointerDest::Input => panic!(),
            PointerDest::Output => &mut self.output,
            PointerDest::Scratch => &mut self.scratch,
            PointerDest::Temp => &mut self.tmp,
        };
        let len = LEN.min(src.len() - p.index);
        src[p.index..p.index + len].copy_from_slice(&v[..len]);
        Ok(())
    }

    pub fn set<const DEST: u8>(&mut self, p: Pointer<DEST>, v: u8) -> Res<()> {
        p.debug(1);
        let dest = match p.dest() {
            PointerDest::Null => None,
            PointerDest::Input => None,
            PointerDest::Output => self.output.get_mut(p.index),
            PointerDest::Scratch => self.scratch.get_mut(p.index),
            PointerDest::Temp => self.tmp.get_mut(p.index),
        }
        .message(|_| format!("Setting byte at {}", p))?;
        *dest = v;
        Ok(())
    }

    pub fn set_bytes<const DEST: u8>(&mut self, p: Pointer<DEST>, v: &[u8]) -> Res<()> {
        p.debug(v.len());
        match p.dest() {
            PointerDest::Null => None,
            PointerDest::Input => None,
            PointerDest::Output => self.output.get_mut(p.index..p.index + v.len()),
            PointerDest::Scratch => self.scratch.get_mut(p.index..p.index + v.len()),
            PointerDest::Temp => self.tmp.get_mut(p.index..p.index + v.len()),
        }
        .message(|_| format!("Writing {} bytes to {}", v.len(), p))?
        .copy_from_slice(v);
        Ok(())
    }

    /// llvm prefetch intrinsic is only made available on x86 (via _mm_prefetch),
    /// for any other arch this function is just a placeholder
    pub fn prefetch<const SRC: u8>(&mut self, p: Pointer<SRC>) {
        let _target = match p.dest() {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input,
            PointerDest::Output => self.output,
            PointerDest::Scratch => self.scratch,
            PointerDest::Temp => self.tmp,
        }
        .get(p.index);

        #[cfg(all(feature = "x86_sse", any(target_arch = "x86", target_arch = "x86_64")))]
        if let Some(v) = _target {
            #[cfg(target_arch = "x86")]
            use core::arch::x86::*;
            #[cfg(target_arch = "x86_64")]
            use core::arch::x86_64::*;

            let addr = (v as *const u8).cast();
            unsafe {
                _mm_prefetch::<{ _MM_HINT_T0 }>(addr);
            }
        }
    }

    /// copies 8 bytes at a time from src into dest, including previously copied bytes if ranges overlap
    pub fn repeat_copy_64<const DEST: u8, const SRC: u8>(
        &mut self,
        dest: Pointer<DEST>,
        src: Pointer<SRC>,
        bytes: usize,
    ) -> Res<()> {
        if dest.dest() != src.dest() || bytes < src.index.abs_diff(dest.index) {
            self.copy_bytes(dest, src, bytes)
        } else {
            dest.debug(bytes);
            let buf: &mut [u8] = match dest.dest() {
                PointerDest::Null => self.raise(format!("{}", dest))?,
                PointerDest::Input => self.raise(format!("{}", dest))?,
                PointerDest::Output => self.output,
                PointerDest::Scratch => self.scratch,
                PointerDest::Temp => self.tmp,
            };
            if src.index.max(dest.index) + bytes > buf.len() {
                Err(ErrorBuilder {
                    #[cfg(feature = "verbose_errors")]
                    message: Some(format!("{}, {}, {}, {}", bytes, src, dest, buf.len())),
                    ..Default::default()
                })?
            }
            let mut n = 0;
            while n < bytes {
                buf.copy_within(src.index + n..src.index + bytes.min(n + 8), dest.index + n);
                n += 8;
            }
            Ok(())
        }
    }

    pub fn copy_64_add<const DEST: u8, const LHS: u8>(
        &mut self,
        dest: Pointer<DEST>,
        lhs: Pointer<LHS>,
        rhs: Pointer<DEST>,
        n: usize,
    ) -> Res<()> {
        if n == 0 {
            return Ok(());
        }
        if rhs.index.abs_diff(dest.index) < 16 || n <= 8 {
            for i in 0..=(n - 1) / 8 {
                let l: u8x16 =
                    bytemuck::cast(u64x2::splat(u64::from_ne_bytes(self.get_arr(lhs + i * 8)?)));
                let r: u8x16 =
                    bytemuck::cast(u64x2::splat(u64::from_ne_bytes(self.get_arr(rhs + i * 8)?)));
                let sum: u64x2 = bytemuck::cast(l + r);
                self.set_arr(dest + i * 8, sum.as_array_ref()[0].to_ne_bytes())?
            }
        } else {
            for i in 0..=(n - 1) / 16 {
                let l = u8x16::from(self.get_arr(lhs + i * 16)?);
                let r = u8x16::from(self.get_arr(rhs + i * 16)?);
                let sum = l + r;
                self.set_arr(dest + i * 16, *sum.as_array_ref())?
            }
        }
        Ok(())
    }

    pub fn copy_bytes<const DEST: u8, const SRC: u8>(
        &mut self,
        dest: Pointer<DEST>,
        src: Pointer<SRC>,
        n: usize,
    ) -> Res<()> {
        dest.debug(n);
        let req_len = src.index.max(dest.index) + n;
        if dest.dest() == src.dest() {
            if dest.index != src.index {
                match dest.dest() {
                    PointerDest::Null => Err(ErrorBuilder::default())?,
                    PointerDest::Input => Err(ErrorBuilder::default())?,
                    PointerDest::Output => {
                        self.assert_le(req_len, self.output.len())?;
                        self.output
                            .copy_within(src.index..src.index + n, dest.index)
                    }
                    PointerDest::Scratch => self
                        .scratch
                        .copy_within(src.index..src.index + n, dest.index),
                    PointerDest::Temp => self.tmp.copy_within(src.index..src.index + n, dest.index),
                }
            }
        } else {
            match dest.dest() {
                PointerDest::Null => Err(ErrorBuilder::default())?,
                PointerDest::Input => Err(ErrorBuilder::default())?,
                PointerDest::Output => self
                    .output
                    .get_mut(dest.index..dest.index + n)
                    .msg_of(&(dest, n))?
                    .copy_from_slice(
                        match src.dest() {
                            PointerDest::Null => None,
                            PointerDest::Input => self.input.get(src.index..src.index + n),
                            PointerDest::Output => None,
                            PointerDest::Scratch => self.scratch.get(src.index..src.index + n),
                            PointerDest::Temp => self.tmp.get(src.index..src.index + n),
                        }
                        .msg_of(&(src, n))?,
                    ),
                PointerDest::Scratch => self.scratch[dest.index..dest.index + n].copy_from_slice(
                    match src.dest() {
                        PointerDest::Null => None,
                        PointerDest::Input => self.input.get(src.index..src.index + n),
                        PointerDest::Output => self.output.get(src.index..src.index + n),
                        PointerDest::Scratch => None,
                        PointerDest::Temp => self.tmp.get(src.index..src.index + n),
                    }
                    .msg_of(&(src, n))?,
                ),
                PointerDest::Temp => self.tmp[dest.index..dest.index + n].copy_from_slice(
                    match src.dest() {
                        PointerDest::Null => None,
                        PointerDest::Input => self.input.get(src.index..src.index + n),
                        PointerDest::Output => self.output.get(src.index..src.index + n),
                        PointerDest::Scratch => self.scratch.get(src.index..src.index + n),
                        PointerDest::Temp => None,
                    }
                    .msg_of(&(src, n))?,
                ),
            }
        }
        Ok(())
    }

    pub fn memset<const DEST: u8>(&mut self, p: Pointer<DEST>, v: u8, n: usize) -> Res<()> {
        p.debug(n);
        match p.dest() {
            PointerDest::Null => Err(ErrorBuilder::default())?,
            PointerDest::Input => Err(ErrorBuilder::default())?,
            PointerDest::Output => self.output.get_mut(p.index..p.index + n).msg_of(&(p, n))?,
            PointerDest::Scratch => &mut self.scratch[p.index..p.index + n],
            PointerDest::Temp => &mut self.tmp[p.index..p.index + n],
        }
        .fill(v);
        Ok(())
    }
}
