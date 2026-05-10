//! Minimal cursor over the decompressed body.
//!
//! All reads are bounds-checked and convert into [`CrabbyError::Detokenize`]
//! with a descriptive context. Kept separate from the frame so unit
//! tests for each decoder can drive a cursor directly.

use crabby_error::{CrabbyError, Result};

/// Read-only cursor tracking a position into a shared byte slice.
#[derive(Debug)]
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub const fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub const fn advance_to(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Read a little-endian `u32` at an explicit offset (does not move pos).
    pub fn read_u32_at(&self, offset: usize) -> Result<u32> {
        let slice = self.slice_at(offset, 4)?;
        Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    /// Read a little-endian `u32` at the current position and advance.
    pub fn read_u32(&mut self) -> Result<u32> {
        let v = self.read_u32_at(self.pos)?;
        self.pos += 4;
        Ok(v)
    }

    /// Peek a single byte at the current position.
    pub fn peek_u8(&self) -> Result<u8> {
        let slice = self.slice_at(self.pos, 1)?;
        Ok(slice[0])
    }

    /// Borrow `len` bytes at the current position and advance.
    pub fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let slice = self.slice_at(self.pos, len)?;
        self.pos += len;
        Ok(slice)
    }

    fn slice_at(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| CrabbyError::Detokenize {
                context: format!("offset {offset} + {len} overflowed"),
                source: "arithmetic overflow".into(),
            })?;
        if end > self.buf.len() {
            return Err(CrabbyError::Detokenize {
                context: format!(
                    "read past end: needed {end} bytes, body has {}",
                    self.buf.len(),
                ),
                source: "short read".into(),
            });
        }
        Ok(&self.buf[offset..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_u32_at_advances_caller_tracking() {
        let bytes = [0x01, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff];
        let mut c = Cursor::new(&bytes);
        assert_eq!(c.read_u32().unwrap(), 1);
        assert_eq!(c.read_u32().unwrap(), u32::MAX);
        assert_eq!(c.remaining(), 0);
    }

    #[test]
    fn read_past_end_errors() {
        let mut c = Cursor::new(&[1, 2]);
        let err = c.read_u32().expect_err("should fail");
        match err {
            CrabbyError::Detokenize { context, .. } => {
                assert!(context.contains("read past end"), "got: {context}");
            }
            other => panic!("expected Detokenize, got {other:?}"),
        }
    }

    #[test]
    fn take_borrows_and_advances() {
        let bytes = b"abcdef";
        let mut c = Cursor::new(bytes);
        assert_eq!(c.take(3).unwrap(), b"abc");
        assert_eq!(c.take(3).unwrap(), b"def");
        assert_eq!(c.remaining(), 0);
    }
}
