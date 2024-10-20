use crate::core::error::{ErrorBuilder, ErrorContext, Res, ResultBuilder, WithContext};
use crate::core::Core;
use std::fmt::{Display, Formatter};
use std::mem::size_of;

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum PointerDest {
    #[default]
    Null,
    Input,
    Output,
    Scratch,
    Temp,
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
pub(crate) struct Pointer {
    pub into: PointerDest,
    pub index: usize,
}

impl Display for Pointer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}[{}]", self.into, self.index)
    }
}

impl ErrorContext for Pointer {}

impl Pointer {
    pub fn input(index: usize) -> Self {
        Pointer {
            into: PointerDest::Input,
            index,
        }
    }
    pub fn output(index: usize) -> Self {
        Pointer {
            into: PointerDest::Output,
            index,
        }
    }
    pub fn scratch(index: usize) -> Self {
        Pointer {
            into: PointerDest::Scratch,
            index,
        }
    }
    pub fn tmp(index: usize) -> Self {
        Pointer {
            into: PointerDest::Temp,
            index,
        }
    }
    pub fn null() -> Pointer {
        Default::default()
    }
    pub fn is_null(&self) -> bool {
        self.into == PointerDest::Null
    }
    pub fn debug(&self, _: usize) {
        // do nothing (there are no bugs)
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

impl std::ops::AddAssign<i32> for Pointer {
    fn add_assign(&mut self, rhs: i32) {
        self.index = self.index.checked_add_signed(rhs as _).unwrap()
    }
}

impl std::ops::SubAssign<i32> for Pointer {
    fn sub_assign(&mut self, rhs: i32) {
        self.index = self.index.checked_add_signed(-rhs as _).unwrap()
    }
}

impl std::ops::Sub<Pointer> for Pointer {
    type Output = Result<usize, ErrorBuilder>;

    fn sub(self, rhs: Pointer) -> Self::Output {
        self.assert_eq(self.into, rhs.into)?;
        self.index
            .checked_sub(rhs.index)
            .msg_of(&(self.index, rhs.index))
    }
}

impl std::ops::Sub<usize> for Pointer {
    type Output = Result<Pointer, ErrorBuilder>;

    fn sub(self, rhs: usize) -> Self::Output {
        self.index
            .checked_sub(rhs)
            .map(|index| Pointer { index, ..self })
            .msg_of(&(self.index, rhs))
    }
}

impl std::ops::Sub<u32> for Pointer {
    type Output = Result<Pointer, ErrorBuilder>;

    fn sub(self, rhs: u32) -> Self::Output {
        self.sub(rhs as usize)
    }
}

impl std::ops::Sub<i32> for Pointer {
    type Output = Result<Pointer, ErrorBuilder>;

    fn sub(self, rhs: i32) -> Self::Output {
        isize::try_from(rhs)
            .at(&self)?
            .checked_neg()
            .and_then(|v| self.index.checked_add_signed(v))
            .map(|index| Pointer { index, ..self })
            .msg_of(&(self.index, rhs))
    }
}

impl Core<'_> {
    pub fn get_byte(&self, p: Pointer) -> Res<u8> {
        Ok(match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input.get(p.index),
            PointerDest::Output => self.output.get(p.index),
            PointerDest::Scratch => self.scratch.get(p.index),
            PointerDest::Temp => self.tmp.get(p.index),
        }
        .copied()
        .msg_of(&p)?)
    }
    pub fn get_slice(&mut self, p: Pointer, n: usize) -> Res<&[u8]> {
        Ok(match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input.get(p.index..p.index + n),
            PointerDest::Output => self.output.get(p.index..p.index + n),
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + n);
                self.scratch.get(p.index..p.index + n)
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + n);
                self.tmp.get(p.index..p.index + n)
            }
        }
        .message(|_| format!("oob {}..{}", p, p.index + n))?)
    }
    pub fn get_le_bytes(&mut self, p: Pointer, n: usize) -> Res<usize> {
        let mut bytes = [0; size_of::<usize>()];
        bytes[..n].copy_from_slice(self.get_slice(p, n)?);
        Ok(usize::from_le_bytes(bytes))
    }
    pub fn get_be_bytes(&mut self, p: Pointer, n: usize) -> Res<usize> {
        const B: usize = size_of::<usize>();
        let mut bytes = [0; B];
        bytes[B - n..].copy_from_slice(self.get_slice(p, n)?);
        Ok(usize::from_be_bytes(bytes))
    }

    pub fn ensure_scratch(&mut self, size: usize) {
        if self.scratch.len() < size {
            self.scratch.resize(size, 0);
        }
    }

    pub fn ensure_tmp(&mut self, size: usize) {
        if self.tmp.len() < size {
            self.tmp.resize(size, 0);
        }
    }

    pub fn set(&mut self, p: Pointer, v: u8) -> Res<()> {
        p.debug(1);
        let dest = match p.into {
            PointerDest::Null => None,
            PointerDest::Input => None,
            PointerDest::Output => self.output.get_mut(p.index),
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + 1);
                self.scratch.get_mut(p.index)
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + 1);
                self.tmp.get_mut(p.index)
            }
        }
        .message(|_| format!("Setting byte at {}", p))?;
        *dest = v;
        Ok(())
    }

    pub fn set_bytes(&mut self, p: Pointer, v: &[u8]) -> Res<()> {
        p.debug(v.len());
        match p.into {
            PointerDest::Null => None,
            PointerDest::Input => None,
            PointerDest::Output => self.output.get_mut(p.index..p.index + v.len()),
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + v.len());
                self.scratch.get_mut(p.index..p.index + v.len())
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + v.len());
                self.tmp.get_mut(p.index..p.index + v.len())
            }
        }
        .message(|_| format!("Writing {} bytes to {}", v.len(), p))?
        .copy_from_slice(v);
        Ok(())
    }

    /// copies 8 bytes at a time from src into dest, including previously copied bytes if ranges overlap
    pub fn repeat_copy_64(&mut self, dest: Pointer, src: Pointer, bytes: usize) -> Res<()> {
        if dest.into != src.into || bytes < src.index.abs_diff(dest.index) {
            self.copy_bytes(dest, src, bytes)
        } else {
            dest.debug(bytes);
            let buf: &mut [u8] = match dest.into {
                PointerDest::Null => self.raise(format!("{}", dest))?,
                PointerDest::Input => self.raise(format!("{}", dest))?,
                PointerDest::Output => self.output,
                PointerDest::Scratch => &mut self.scratch,
                PointerDest::Temp => &mut self.tmp,
            };
            if src.index.max(dest.index) + bytes > buf.len() {
                Err(ErrorBuilder {
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

    pub fn copy_64_add(&mut self, dest: Pointer, lhs: Pointer, rhs: Pointer, n: usize) -> Res<()> {
        for i in 0..n {
            self.set(
                dest + i,
                self.get_byte(lhs + i)?
                    .wrapping_add(self.get_byte(rhs + i)?),
            )
            .at(self)?
        }
        Ok(())
    }

    pub fn copy_bytes(&mut self, dest: Pointer, src: Pointer, n: usize) -> Res<()> {
        dest.debug(n);
        let req_len = src.index.max(dest.index) + n;
        if dest.into == src.into {
            if dest.index != src.index {
                match dest.into {
                    PointerDest::Null => Err(ErrorBuilder::default())?,
                    PointerDest::Input => Err(ErrorBuilder::default())?,
                    PointerDest::Output => {
                        self.assert_le(req_len, self.output.len())?;
                        self.output
                            .copy_within(src.index..src.index + n, dest.index)
                    }
                    PointerDest::Scratch => {
                        self.ensure_scratch(req_len);
                        self.scratch
                            .copy_within(src.index..src.index + n, dest.index)
                    }
                    PointerDest::Temp => {
                        self.ensure_tmp(req_len);
                        self.tmp.copy_within(src.index..src.index + n, dest.index)
                    }
                }
            }
        } else {
            match dest.into {
                PointerDest::Null => Err(ErrorBuilder::default())?,
                PointerDest::Input => Err(ErrorBuilder::default())?,
                PointerDest::Output => self
                    .output
                    .get_mut(dest.index..dest.index + n)
                    .msg_of(&(dest, n))?
                    .copy_from_slice(
                        match src.into {
                            PointerDest::Null => None,
                            PointerDest::Input => self.input.get(src.index..src.index + n),
                            PointerDest::Output => None,
                            PointerDest::Scratch => self.scratch.get(src.index..src.index + n),
                            PointerDest::Temp => self.tmp.get(src.index..src.index + n),
                        }
                        .msg_of(&(src, n))?,
                    ),
                PointerDest::Scratch => {
                    self.ensure_scratch(dest.index + n);
                    self.scratch[dest.index..dest.index + n].copy_from_slice(
                        match src.into {
                            PointerDest::Null => None,
                            PointerDest::Input => self.input.get(src.index..src.index + n),
                            PointerDest::Output => self.output.get(src.index..src.index + n),
                            PointerDest::Scratch => None,
                            PointerDest::Temp => self.tmp.get(src.index..src.index + n),
                        }
                        .msg_of(&(src, n))?,
                    )
                }
                PointerDest::Temp => {
                    self.ensure_tmp(dest.index + n);
                    self.tmp[dest.index..dest.index + n].copy_from_slice(
                        match src.into {
                            PointerDest::Null => None,
                            PointerDest::Input => self.input.get(src.index..src.index + n),
                            PointerDest::Output => self.output.get(src.index..src.index + n),
                            PointerDest::Scratch => self.scratch.get(src.index..src.index + n),
                            PointerDest::Temp => None,
                        }
                        .msg_of(&(src, n))?,
                    )
                }
            }
        }
        Ok(())
    }

    pub fn memset(&mut self, p: Pointer, v: u8, n: usize) -> Res<()> {
        p.debug(n);
        match p.into {
            PointerDest::Null => Err(ErrorBuilder::default())?,
            PointerDest::Input => Err(ErrorBuilder::default())?,
            PointerDest::Output => self.output.get_mut(p.index..p.index + n).msg_of(&(p, n))?,
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + n);
                &mut self.scratch[p.index..p.index + n]
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + n);
                &mut self.tmp[p.index..p.index + n]
            }
        }
        .fill(v);
        Ok(())
    }
}
