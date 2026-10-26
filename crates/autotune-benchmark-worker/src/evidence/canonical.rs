//! Ordered value model and compact canonical writer.
//!
//! Objects keep insertion order and the writer never sorts. Integers are emitted
//! as quoted canonical decimal strings, so the generic reader recovers text and
//! typed accessors accept both those strings and their own typed variants.

/// Error returned when inserting a field that already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectError {
    DuplicateKey,
}

/// Error returned by the exact-field helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// The object has a different number of fields.
    Count,
    /// A field is missing, unknown or in the wrong position.
    Name,
}

/// Error returned by a typed integer or ratio accessor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessError {
    NotText,
    Empty,
    Sign,
    NegativeZero,
    LeadingZero,
    Digit,
    Overflow,
    NotRatio,
    Fields,
    Denominator,
}

/// Error returned when constructing a ratio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatioError {
    Denominator,
}

/// Exact numerator/denominator ratio with a positive denominator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    num: i64,
    den: u64,
}

impl Ratio {
    pub fn new(num: i64, den: u64) -> Result<Self, RatioError> {
        if den == 0 {
            return Err(RatioError::Denominator);
        }
        Ok(Self { num, den })
    }
    pub fn num(self) -> i64 {
        self.num
    }
    pub fn den(self) -> u64 {
        self.den
    }
}

/// Canonical value. `Text` is the only representation the reader can recover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Text(String),
    U64(u64),
    I64(i64),
    Ratio(Ratio),
    Array(Vec<Value>),
    Object(Object),
}

impl Value {
    /// Encode this value as compact canonical UTF-8 JSON.
    pub fn to_canonical(&self) -> String {
        encode(self)
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(object) => Some(object),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }
    /// Read the unsigned value of a `U64`, a non-negative `I64` or a canonical
    /// unsigned decimal string.
    pub fn as_u64(&self) -> Result<u64, AccessError> {
        match self {
            Self::U64(number) => Ok(*number),
            Self::I64(number) => u64::try_from(*number).map_err(|_| AccessError::Sign),
            other => parse_u64(other.as_text().ok_or(AccessError::NotText)?),
        }
    }
    /// Read the signed value of an `I64`, a `U64` that fits or a canonical signed
    /// decimal string.
    pub fn as_i64(&self) -> Result<i64, AccessError> {
        match self {
            Self::I64(number) => Ok(*number),
            Self::U64(number) => i64::try_from(*number).map_err(|_| AccessError::Overflow),
            other => parse_i64(other.as_text().ok_or(AccessError::NotText)?),
        }
    }
    /// Read a `Ratio` directly, or parse the exact
    /// `{"num":"<i64>","den":"<u64>"}` ratio form.
    pub fn as_ratio(&self) -> Result<Ratio, AccessError> {
        if let Self::Ratio(ratio) = self {
            return Ok(*ratio);
        }
        let object = self.as_object().ok_or(AccessError::NotRatio)?;
        object
            .require_exact(&["num", "den"])
            .map_err(|_| AccessError::Fields)?;
        let Some(num) = object.get("num") else {
            return Err(AccessError::Fields);
        };
        let Some(den) = object.get("den") else {
            return Err(AccessError::Fields);
        };
        Ratio::new(num.as_i64()?, den.as_u64()?).map_err(|_| AccessError::Denominator)
    }
}

/// Ordered object with duplicate-key rejection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Object {
    entries: Vec<(String, Value)>,
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }
    /// Append a field; an existing key is a typed error and leaves the object unchanged.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Result<(), ObjectError> {
        let key = key.into();
        if self.entries.iter().any(|(existing, _)| existing == &key) {
            return Err(ObjectError::DuplicateKey);
        }
        self.entries.push((key, value));
        Ok(())
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(existing, _)| existing.as_str() == key)
            .map(|(_, value)| value)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }
    /// Require exactly these field names, in this order.
    pub fn require_exact(&self, keys: &[&str]) -> Result<(), FieldError> {
        if self.entries.len() != keys.len() {
            return Err(FieldError::Count);
        }
        for ((key, _), expected) in self.entries.iter().zip(keys) {
            if key.as_str() != *expected {
                return Err(FieldError::Name);
            }
        }
        Ok(())
    }
    /// Copy this object without the named top-level field, e.g. a document's own
    /// digest. Nested fields are untouched and an absent key is a silent no-op.
    pub fn without(&self, key: &str) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .filter(|(existing, _)| existing.as_str() != key)
                .cloned()
                .collect(),
        }
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// Encode a value as compact canonical UTF-8 JSON.
pub fn encode(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Text(text) => write_text(out, text),
        Value::U64(number) => {
            out.push('"');
            out.push_str(&number.to_string());
            out.push('"');
        }
        Value::I64(number) => {
            out.push('"');
            out.push_str(&number.to_string());
            out.push('"');
        }
        Value::Ratio(ratio) => {
            out.push_str("{\"num\":\"");
            out.push_str(&ratio.num.to_string());
            out.push_str("\",\"den\":\"");
            out.push_str(&ratio.den.to_string());
            out.push_str("\"}");
        }
        Value::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, value);
            }
            out.push(']');
        }
        Value::Object(object) => {
            out.push('{');
            for (index, (key, value)) in object.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_text(out, key);
                out.push(':');
                write_value(out, value);
            }
            out.push('}');
        }
    }
}

fn write_text(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if (character as u32) < 0x20 => {
                out.push_str("\\u00");
                let byte = character as u8;
                out.push(HEX[usize::from(byte >> 4)] as char);
                out.push(HEX[usize::from(byte & 0x0f)] as char);
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

fn parse_u64(text: &str) -> Result<u64, AccessError> {
    let bytes = text.as_bytes();
    let Some(first) = bytes.first() else {
        return Err(AccessError::Empty);
    };
    if *first == b'+' {
        return Err(AccessError::Sign);
    }
    if *first == b'-' {
        return Err(if bytes.len() == 1 {
            AccessError::Digit
        } else {
            AccessError::Sign
        });
    }
    if *first == b'0' && bytes.len() > 1 {
        return Err(AccessError::LeadingZero);
    }
    if !bytes.iter().all(u8::is_ascii_digit) {
        return Err(AccessError::Digit);
    }
    text.parse::<u64>().map_err(|_| AccessError::Overflow)
}

fn parse_i64(text: &str) -> Result<i64, AccessError> {
    let bytes = text.as_bytes();
    let Some(first) = bytes.first() else {
        return Err(AccessError::Empty);
    };
    let digits = if *first == b'+' {
        return Err(AccessError::Sign);
    } else if *first == b'-' {
        let digits = &bytes[1..];
        if digits.is_empty() {
            return Err(AccessError::Digit);
        }
        if digits[0] == b'0' {
            return Err(if digits.len() == 1 {
                AccessError::NegativeZero
            } else {
                AccessError::LeadingZero
            });
        }
        digits
    } else {
        bytes
    };
    if digits[0] == b'0' && digits.len() > 1 {
        return Err(AccessError::LeadingZero);
    }
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(AccessError::Digit);
    }
    text.parse::<i64>().map_err(|_| AccessError::Overflow)
}
