use crate::core::Core;

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum PointerDest {
    #[default]
    Null,
    Input,
    Output,
    Scratch,
    Temp,
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
pub struct Pointer {
    pub into: PointerDest,
    pub index: usize,
}

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
    pub fn align(&self, align: usize) -> Pointer {
        Pointer {
            index: (self.index + (align - 1)) & !(align - 1),
            ..*self
        }
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
    type Output = usize;

    fn sub(self, rhs: Pointer) -> Self::Output {
        assert_eq!(self.into, rhs.into);
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

impl std::ops::Sub<u32> for Pointer {
    type Output = Pointer;

    fn sub(self, rhs: u32) -> Self::Output {
        self.sub(rhs as usize)
    }
}

impl std::ops::Sub<i32> for Pointer {
    type Output = Pointer;

    fn sub(self, rhs: i32) -> Self::Output {
        Pointer {
            index: self
                .index
                .checked_add_signed(-isize::try_from(rhs).unwrap())
                .unwrap(),
            ..self
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd)]
pub struct IntPointer {
    pub into: PointerDest,
    pub index: usize,
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
        assert_eq!(self.into, rhs.into);
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

impl From<IntPointer> for Pointer {
    fn from(value: IntPointer) -> Self {
        Pointer {
            into: value.into,
            index: value.index,
        }
    }
}

impl Core<'_> {
    pub fn new<'a>(input: &'a [u8], output: &'a mut [u8]) -> Core<'a> {
        Core {
            input,
            output,
            scratch: Vec::new(),
            tmp: Vec::new(),
        }
    }
    pub fn get_byte(&self, p: Pointer) -> u8 {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => self.input[p.index],
            PointerDest::Output => self.output[p.index],
            PointerDest::Scratch => self.scratch[p.index],
            PointerDest::Temp => self.tmp[p.index],
        }
    }
    pub fn get_as_usize(&self, p: Pointer) -> usize {
        self.get_byte(p) as usize
    }
    pub fn get_as_bool(&self, p: Pointer) -> bool {
        self.get_byte(p) != 0
    }
    pub fn get_slice(&mut self, p: Pointer, n: usize) -> &[u8] {
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => &self.input[p.index..p.index + n],
            PointerDest::Output => &self.output[p.index..p.index + n],
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + n);
                &self.scratch[p.index..p.index + n]
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + n);
                &self.tmp[p.index..p.index + n]
            }
        }
    }
    pub fn get_bytes_as_usize_le(&mut self, p: Pointer, n: usize) -> usize {
        let mut bytes = [0; size_of::<usize>()];
        bytes[..n].copy_from_slice(self.get_slice(p, n));
        usize::from_le_bytes(bytes)
    }
    pub fn get_bytes_as_usize_be(&mut self, p: Pointer, n: usize) -> usize {
        const B: usize = size_of::<usize>();
        let mut bytes = [0; B];
        bytes[B - n..].copy_from_slice(self.get_slice(p, n));
        usize::from_be_bytes(bytes)
    }

    pub fn get_int(&mut self, p: IntPointer) -> i32 {
        i32::from_le_bytes(self.get_slice(Pointer::from(p), 4).try_into().unwrap())
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

    pub fn set(&mut self, p: Pointer, v: u8) {
        p.debug(1);
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => panic!(),
            PointerDest::Output => self.output[p.index] = v,
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + 1);
                self.scratch[p.index] = v
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + 1);
                self.tmp[p.index] = v
            }
        }
    }

    pub fn set_int(&mut self, p: IntPointer, v: i32) {
        Pointer::from(p).debug(4);
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => panic!(),
            PointerDest::Output => {
                self.output[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + 4);
                self.scratch[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + 4);
                self.tmp[p.index..p.index + 4].copy_from_slice(&v.to_le_bytes())
            }
        }
    }

    pub fn set_bytes(&mut self, p: Pointer, v: &[u8]) {
        p.debug(v.len());
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => panic!(),
            PointerDest::Output => self.output[p.index..p.index + v.len()].copy_from_slice(v),
            PointerDest::Scratch => {
                self.ensure_scratch(p.index + v.len());
                self.scratch[p.index..p.index + v.len()].copy_from_slice(v)
            }
            PointerDest::Temp => {
                self.ensure_tmp(p.index + v.len());
                self.tmp[p.index..p.index + v.len()].copy_from_slice(v)
            }
        }
    }

    /// copies 8 bytes at a time from src into dest, including previously copied bytes if ranges overlap
    pub fn repeat_copy_64(&mut self, dest: Pointer, src: Pointer, bytes: usize) {
        if dest.into != src.into {
            self.memmove(dest, src, bytes);
        } else {
            dest.debug(bytes);
            let buf: &mut [u8] = match dest.into {
                PointerDest::Null => panic!(),
                PointerDest::Input => panic!(),
                PointerDest::Output => self.output,
                PointerDest::Scratch => {
                    self.ensure_scratch(dest.index + bytes);
                    &mut self.scratch
                }
                PointerDest::Temp => {
                    self.ensure_tmp(dest.index + bytes);
                    &mut self.tmp
                }
            };
            let mut n = 0;
            while n < bytes {
                buf.copy_within(src.index + n..src.index + bytes.min(n + 8), dest.index + n);
                n += 8;
            }
        }
    }

    pub fn copy_64_bytes(&mut self, dest: Pointer, src: Pointer) {
        self.memmove(dest, src, 64)
    }

    pub fn copy_64_add(&mut self, dest: Pointer, lhs: Pointer, rhs: Pointer, n: usize) {
        for i in 0..n {
            self.set(
                dest + i,
                self.get_byte(lhs + i).wrapping_add(self.get_byte(rhs + i)),
            )
        }
    }

    pub fn memcpy(&mut self, dest: Pointer, src: Pointer, n: usize) {
        self.memmove(dest, src, n)
    }

    pub fn memmove(&mut self, dest: Pointer, src: Pointer, n: usize) {
        dest.debug(n);
        if dest.into == src.into {
            if dest.index != src.index {
                match dest.into {
                    PointerDest::Null => panic!(),
                    PointerDest::Input => panic!(),
                    PointerDest::Output => self
                        .output
                        .copy_within(src.index..src.index + n, dest.index),
                    PointerDest::Scratch => {
                        self.ensure_scratch(dest.index + n);
                        self.scratch
                            .copy_within(src.index..src.index + n, dest.index)
                    }
                    PointerDest::Temp => {
                        self.ensure_tmp(dest.index + n);
                        self.tmp.copy_within(src.index..src.index + n, dest.index)
                    }
                }
            }
        } else {
            match dest.into {
                PointerDest::Null => panic!(),
                PointerDest::Input => panic!(),
                PointerDest::Output => {
                    self.output[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => &self.input[src.index..src.index + n],
                        PointerDest::Output => panic!(),
                        PointerDest::Scratch => &self.scratch[src.index..src.index + n],
                        PointerDest::Temp => &self.tmp[src.index..src.index + n],
                    })
                }
                PointerDest::Scratch => {
                    self.ensure_scratch(dest.index + n);
                    self.scratch[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => &self.input[src.index..src.index + n],
                        PointerDest::Output => &self.output[src.index..src.index + n],
                        PointerDest::Scratch => panic!(),
                        PointerDest::Temp => &self.tmp[src.index..src.index + n],
                    })
                }
                PointerDest::Temp => {
                    self.ensure_tmp(dest.index + n);
                    self.tmp[dest.index..dest.index + n].copy_from_slice(match src.into {
                        PointerDest::Null => panic!(),
                        PointerDest::Input => &self.input[src.index..src.index + n],
                        PointerDest::Output => &self.output[src.index..src.index + n],
                        PointerDest::Scratch => &self.scratch[src.index..src.index + n],
                        PointerDest::Temp => panic!(),
                    })
                }
            }
        }
    }

    pub fn memset(&mut self, p: Pointer, v: u8, n: usize) {
        p.debug(n);
        match p.into {
            PointerDest::Null => panic!(),
            PointerDest::Input => panic!(),
            PointerDest::Output => &mut self.output[p.index..p.index + n],
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
    }
}
