mod bitknit;
mod kraken;
mod leviathan;
mod lzna;
mod mermaid;

use crate::decoder::pointer::{Pointer, PointerDest};
use crate::decoder::Core;
use crate::ooz::error::Res;
use std::fmt::Debug;

pub(crate) use bitknit::*;
pub(crate) use kraken::Kraken;
pub(crate) use leviathan::Leviathan;
pub(crate) use lzna::*;
pub(crate) use mermaid::Mermaid;

pub trait Algorithm: Debug {
    fn process(
        &self,
        core: &mut Core,
        mode: usize,
        src: Pointer<{ PointerDest::INPUT }>,
        src_used: usize,
        dst_start: Pointer<{ PointerDest::OUTPUT }>,
        dst: Pointer<{ PointerDest::OUTPUT }>,
        dst_size: usize,
    ) -> Res<()>;
}
