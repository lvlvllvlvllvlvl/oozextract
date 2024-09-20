use crate::core::Core;
use crate::error::OozError;
use crate::pointer::Pointer;

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
    ) -> Result<(), OozError>;
}
