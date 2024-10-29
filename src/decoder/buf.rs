use crate::ooz;

pub(crate) trait SafeBuf: bytes::Buf {
    fn get_byte(&mut self) -> ooz::error::Res<u8> {
        self.re
    }
}
