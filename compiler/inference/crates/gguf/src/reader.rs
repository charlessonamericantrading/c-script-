use crate::error::{GgufError, Result};

/// A cursor over an in-memory GGUF file.
///
/// GGUF has no framing beyond "read fields in order" — there is nothing to
/// recover from once a read goes out of bounds, so every primitive read
/// bounds-checks explicitly rather than relying on a panic. This is the one
/// place in the crate allowed to index raw bytes; everything above this
/// module goes through here.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(GgufError::Overflow("read cursor"))?;
        if end > self.buf.len() {
            return Err(GgufError::UnexpectedEof { wanted: n, at: self.pos, len: self.buf.len() });
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.take(1)?[0] as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn bool(&mut self) -> Result<bool> {
        // The spec stores bool as a single byte; llama.cpp writers have at
        // various points emitted 0/1 as well as stray non-0/1 bytes for
        // this field, so treat "nonzero" as true rather than rejecting.
        Ok(self.u8()? != 0)
    }

    /// GGUF strings are `{ uint64 len; uint8 bytes[len] }` — length-prefixed,
    /// no null terminator.
    pub fn gguf_string(&mut self) -> Result<String> {
        let len = self.u64()?;
        let len = usize::try_from(len).map_err(|_| GgufError::Overflow("string length"))?;
        let at = self.pos;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| GgufError::InvalidUtf8 { at })
    }
}
