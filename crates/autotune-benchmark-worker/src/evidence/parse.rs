//! Bounded strict reader with canonicality check.
//!
//! The reader accepts only `{`, `[`, `"`, `true`, `false` and `null` values; any
//! number token at a value position is rejected. A document is accepted only if
//! re-encoding reproduces the input bytes exactly.

use super::canonical::{encode, Object, Value};

/// Maximum number of nested containers accepted by the reader.
pub const MAX_DEPTH: usize = 32;

/// Typed rejection reason. No partially accepted document escapes the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Oversize,
    Truncated,
    Trailing,
    Depth,
    Utf8,
    DuplicateKey,
    Escape,
    Surrogate,
    Control,
    Number,
    Syntax,
    Canonical,
}

/// Decode bounded canonical bytes with a caller-supplied maximum length.
pub fn decode(input: &[u8], max_bytes: usize) -> Result<Value, ParseError> {
    if input.len() > max_bytes {
        return Err(ParseError::Oversize);
    }
    let text = std::str::from_utf8(input).map_err(|_| ParseError::Utf8)?;
    let mut reader = Reader {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = reader.value(0)?;
    if reader.pos != reader.bytes.len() {
        return Err(ParseError::Trailing);
    }
    if encode(&value).as_bytes() != input {
        return Err(ParseError::Canonical);
    }
    Ok(value)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn next(&mut self) -> Result<u8, ParseError> {
        let byte = self.peek().ok_or(ParseError::Truncated)?;
        self.pos += 1;
        Ok(byte)
    }
    fn expect(&mut self, expected: u8) -> Result<(), ParseError> {
        if self.next()? != expected {
            return Err(ParseError::Syntax);
        }
        Ok(())
    }
    fn value(&mut self, depth: usize) -> Result<Value, ParseError> {
        match self.peek().ok_or(ParseError::Truncated)? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => Ok(Value::Text(self.string()?)),
            b't' => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            b'f' => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            b'n' => {
                self.literal(b"null")?;
                Ok(Value::Null)
            }
            b'0'..=b'9' | b'-' | b'+' | b'.' => Err(ParseError::Number),
            _ => Err(ParseError::Syntax),
        }
    }
    fn object(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(ParseError::Depth);
        }
        self.expect(b'{')?;
        let mut object = Object::new();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(object));
        }
        loop {
            match self.peek() {
                Some(b'"') => {}
                Some(_) => return Err(ParseError::Syntax),
                None => return Err(ParseError::Truncated),
            }
            let key = self.string()?;
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            object
                .insert(key, value)
                .map_err(|_| ParseError::DuplicateKey)?;
            match self.next()? {
                b',' => {}
                b'}' => return Ok(Value::Object(object)),
                _ => return Err(ParseError::Syntax),
            }
        }
    }
    fn array(&mut self, depth: usize) -> Result<Value, ParseError> {
        if depth >= MAX_DEPTH {
            return Err(ParseError::Depth);
        }
        self.expect(b'[')?;
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.value(depth + 1)?);
            match self.next()? {
                b',' => {}
                b']' => return Ok(Value::Array(values)),
                _ => return Err(ParseError::Syntax),
            }
        }
    }
    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next()? {
                b'"' => return String::from_utf8(out).map_err(|_| ParseError::Utf8),
                b'\\' => self.escape(&mut out)?,
                0x00..=0x1f => return Err(ParseError::Control),
                byte => out.push(byte),
            }
        }
    }
    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), ParseError> {
        match self.next()? {
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'/' => out.push(b'/'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                let first = self.hex4()?;
                let character = match first {
                    0xd800..=0xdbff => {
                        if self.next()? != b'\\' || self.next()? != b'u' {
                            return Err(ParseError::Surrogate);
                        }
                        let second = self.hex4()?;
                        if !(0xdc00..=0xdfff).contains(&second) {
                            return Err(ParseError::Surrogate);
                        }
                        let combined = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                        char::from_u32(combined).ok_or(ParseError::Surrogate)?
                    }
                    0xdc00..=0xdfff => return Err(ParseError::Surrogate),
                    _ => char::from_u32(first).ok_or(ParseError::Surrogate)?,
                };
                let mut buffer = [0u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            _ => return Err(ParseError::Escape),
        }
        Ok(())
    }
    fn hex4(&mut self) -> Result<u32, ParseError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let digit = match self.next()? {
                byte @ b'0'..=b'9' => u32::from(byte - b'0'),
                byte @ b'a'..=b'f' => u32::from(byte - b'a' + 10),
                byte @ b'A'..=b'F' => u32::from(byte - b'A' + 10),
                _ => return Err(ParseError::Escape),
            };
            value = value * 16 + digit;
        }
        Ok(value)
    }
    fn literal(&mut self, expected: &[u8]) -> Result<(), ParseError> {
        for &byte in expected {
            if self.next()? != byte {
                return Err(ParseError::Syntax);
            }
        }
        Ok(())
    }
}
