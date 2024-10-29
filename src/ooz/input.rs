use crate::ooz::error::{Res, ResultBuilder};
#[cfg(feature = "async")]
use futures::{Stream, StreamExt};
use std::io::Read;
#[cfg(feature = "tokio")]
use tokio::io::{AsyncRead, AsyncReadExt};

pub trait Input<S: AsRef<[u8]>> {
    async fn read_to(&mut self, buf: &mut [u8]) -> Res<()>;
    async fn read_slice(&mut self, buf: &mut bytes::BytesMut, len: usize) -> Res<S>;
    async fn read_array<const N: usize>(&mut self, to_read: usize) -> Res<[u8; N]> {
        let mut buf = [0; N];
        self.read_to(&mut buf[N - to_read..]).await?;
        Ok(buf)
    }
}

impl<R: Read> Input<bytes::BytesMut> for R {
    async fn read_to(&mut self, buf: &mut [u8]) -> Res<()> {
        Ok(self.read_exact(buf)?)
    }

    async fn read_slice(&mut self, buf: &mut bytes::BytesMut, len: usize) -> Res<bytes::BytesMut> {
        if buf.len() < len {
            buf.resize(len, 0);
        }
        let mut slice = buf.split_to(len);
        self.read_exact(slice.as_mut())?;
        Ok(slice)
    }
}

pub(crate) struct Slice<'a> {
    pub(crate) buf: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Input<&'a [u8]> for Slice<'a> {
    async fn read_to(&mut self, buf: &mut [u8]) -> Res<()> {
        buf.copy_from_slice(self.buf.get(self.pos..self.pos + buf.len()).msg_of(&(
            self.pos,
            self.buf,
            buf.len(),
        ))?);
        self.pos += buf.len();
        Ok(())
    }

    async fn read_slice(&mut self, _: &mut bytes::BytesMut, len: usize) -> Res<&'a [u8]> {
        let slice = self
            .buf
            .get(self.pos..self.pos + len)
            .msg_of(&(self.pos, self.buf, len))?;
        self.pos += len;
        Ok(slice)
    }
}

#[cfg(feature = "tokio")]
pub(crate) struct Async<R: AsyncRead + Unpin>(pub(crate) R);

#[cfg(feature = "tokio")]
impl<R: AsyncRead + Unpin> Input<bytes::BytesMut> for Async<R> {
    async fn read_to(&mut self, buf: &mut [u8]) -> Res<()> {
        self.0.read_exact(buf).await?;
        Ok(())
    }

    async fn read_slice(&mut self, buf: &mut bytes::BytesMut, len: usize) -> Res<bytes::BytesMut> {
        if buf.len() < len {
            buf.resize(len, 0);
        }
        let mut slice = buf.split_to(len);
        self.0.read_exact(slice.as_mut()).await?;
        Ok(slice)
    }
}

#[cfg(feature = "async")]
pub(crate) struct ByteStream<
    E: 'static + std::error::Error + Send + Sync,
    S: Stream<Item = Result<bytes::Bytes, E>>,
> {
    pub(crate) current: Option<bytes::Bytes>,
    pub(crate) stream: S,
}

#[cfg(feature = "async")]
impl<
        E: 'static + std::error::Error + Send + Sync,
        S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    > Input<bytes::Bytes> for ByteStream<E, S>
{
    async fn read_to(&mut self, mut out: &mut [u8]) -> Res<()> {
        use crate::ooz::error::ErrorBuilder;
        if self.current.is_none() {
            self.current = ErrorBuilder::invert(self.stream.next().await)?
        }
        while let Some(ref mut bytes) = self.current {
            if out.is_empty() {
                return Ok(());
            } else if bytes.len() >= out.len() {
                out.copy_from_slice(bytes.split_to(out.len()).as_ref());
                if bytes.is_empty() {
                    self.current = None;
                }
                return Ok(());
            } else {
                out[..bytes.len()].copy_from_slice(bytes);
                out = &mut out[bytes.len()..];
                self.current = ErrorBuilder::invert(self.stream.next().await)?;
            }
        }
        Err(ErrorBuilder {
            #[cfg(feature = "verbose_errors")]
            message: Some("Unexpected end of stream".into()),
            ..Default::default()
        })?
    }

    async fn read_slice(&mut self, buf: &mut bytes::BytesMut, len: usize) -> Res<bytes::Bytes> {
        use crate::ooz::error::ErrorBuilder;
        if self.current.is_none() {
            self.current = ErrorBuilder::invert(self.stream.next().await)?;
        }
        if let Some(ref mut bytes) = self.current {
            if bytes.len() >= len {
                return Ok(bytes.split_to(len));
            }
        }

        if buf.len() < len {
            buf.resize(len, 0);
        }
        let mut slice = buf.split_to(len);
        self.read_to(slice.as_mut()).await?;
        Ok(slice.freeze())
    }
}
