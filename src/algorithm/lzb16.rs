use crate::ooz::error::{ErrorContext, Res};

/// Oodle's LZB16 (decoder type 2): byte-aligned LZ77 with 16-bit distances, no entropy coding, in blocks of
/// [`crate::ooz::SMALL_BLOCK`] bytes.
///
/// A command is a control byte (literal count in the low nibble, match code in the high nibble), the literals, then
/// the match:
/// - literal count 15 adds a length extension;
/// - match code 0-14: length `code + 4`, two-byte little-endian distance;
/// - match code 15: a byte `e`. `1LLLLDDD`: length `LLLL + 4` (19 adds an extension), distance `DDD`. Otherwise
///   length `19 + e` and a two-byte distance; `e == 127` adds an extension after the distance.
///
/// An extension is one byte, or `((first & 0x3F) << 8) | second` when the first is 0xC0 or more. A final literal run is
/// cut to the output.
#[derive(Debug)]
pub(crate) struct Lzb16<'a> {
    input: &'a [u8],
    out: &'a mut [u8],
    offset: usize,
    history: usize,
}

impl ErrorContext for Lzb16<'_> {
    fn describe(&self) -> Option<String> {
        Some("Lzb16".into())
    }
}

impl<'a> Lzb16<'a> {
    /// Decode `input` into `out[offset..]`; matches reach back to `out[history..]`.
    pub fn new(input: &'a [u8], out: &'a mut [u8], offset: usize, history: usize) -> Self {
        Lzb16 {
            input,
            out,
            offset,
            history,
        }
    }

    fn byte(&self, pos: &mut usize) -> Res<u8> {
        match self.input.get(*pos).copied() {
            Some(b) => {
                *pos += 1;
                Ok(b)
            }
            None => Ok(self.raise("input ends inside a command".into())?),
        }
    }

    fn extension(&self, pos: &mut usize) -> Res<usize> {
        let first = self.byte(pos)?;
        if first < 0xC0 {
            Ok(usize::from(first))
        } else {
            let second = self.byte(pos)?;
            Ok((usize::from(first & 0x3F) << 8) | usize::from(second))
        }
    }

    /// Decodes the block; returns the input bytes it used.
    pub fn decode(&mut self) -> Res<usize> {
        let end = self.out.len();
        let mut at = self.offset;
        let mut pos = 0;
        while at < end {
            let control = self.byte(&mut pos)?;
            let mut literals = usize::from(control & 0xF);
            if literals == 15 {
                literals += self.extension(&mut pos)?;
            }
            let literals = literals.min(end - at);
            let Some(from) = self.input.get(pos..pos + literals) else {
                return Ok(self.raise("input ends inside a literal run".into())?);
            };
            let Some(to) = self.out.get_mut(at..at + literals) else {
                return Ok(self.raise("literal run past the output".into())?);
            };
            to.copy_from_slice(from);
            pos += literals;
            at += literals;
            if at == end {
                break;
            }
            let code = control >> 4;
            let (length, distance) = if code < 15 {
                let low = self.byte(&mut pos)?;
                let high = self.byte(&mut pos)?;
                (
                    usize::from(code) + 4,
                    usize::from(low) | (usize::from(high) << 8),
                )
            } else {
                let e = self.byte(&mut pos)?;
                if e & 0x80 != 0 {
                    let mut length = usize::from((e >> 3) & 0xF) + 4;
                    if length == 19 {
                        length += self.extension(&mut pos)?;
                    }
                    (length, usize::from(e & 7))
                } else {
                    let low = self.byte(&mut pos)?;
                    let high = self.byte(&mut pos)?;
                    let mut length = 19 + usize::from(e);
                    if e == 127 {
                        length += self.extension(&mut pos)?;
                    }
                    (length, usize::from(low) | (usize::from(high) << 8))
                }
            };
            self.assert_le(1, distance)?;
            self.assert_le(distance, at - self.history)?;
            self.assert_le(length, end - at)?;
            for i in at..at + length {
                let Some(&b) = self.out.get(i - distance) else {
                    return Ok(self.raise("match before the output".into())?);
                };
                if let Some(o) = self.out.get_mut(i) {
                    *o = b;
                }
            }
            at += length;
        }
        Ok(pos)
    }
}
