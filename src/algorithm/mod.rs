mod bitknit;
mod kraken;
mod leviathan;
mod lzna;
mod mermaid;

use crate::core::error::Res;
use crate::core::pointer::Pointer;
use crate::core::Core;

pub(crate) use bitknit::*;
pub(crate) use kraken::Kraken;
pub(crate) use leviathan::Leviathan;
pub(crate) use lzna::*;
pub(crate) use mermaid::Mermaid;

pub trait Algorithm {
    fn process(
        &self,
        core: &mut Core,
        mode: usize,
        src: Pointer,
        src_used: usize,
        dst_start: Pointer,
        dst: Pointer,
        dst_size: usize,
    ) -> Res<()>;
}
