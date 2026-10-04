//! JSON text, as in RFC 8259.
//!
//! [`parse`] turns UTF-8 bytes into a [`Value`]. [`to_vec`] writes that value
//! back out as compact UTF-8. Reading files and talking to the network happen
//! in other crates.
//!
//! A number is stored as the digits from the input. [`Value::as_i64`],
//! [`Value::as_u64`], and [`Value::as_f64`] interpret those digits. If a helper
//! cannot, the digits stay as they were. Two numbers are equal when their
//! digits are equal.

#![deny(unsafe_code)]

mod parse;
mod write;

pub use parse::parse;
pub use write::to_vec;

/// Largest accepted body, in bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Deepest accepted nesting of arrays and objects.
pub const MAX_DEPTH: u32 = 128;

/// A JSON value. Object pairs keep parse order, with the last duplicate name kept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    /// JSON null.
    Null,
    /// JSON true or false.
    Bool(bool),
    /// The original number text, such as `1.0` or `1e2`.
    Number(String),
    /// A Unicode string.
    String(String),
    /// A sequence of values.
    Array(Vec<Value>),
    /// `(name, value)` pairs in parse order.
    Object(Vec<(String, Value)>),
}

/// A parse failure. `line` and `column` are 1-based.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    /// What failed.
    pub message: String,
    /// Byte offset into the input.
    pub offset: usize,
    /// 1-based line.
    pub line: u32,
    /// 1-based column, counting Unicode scalar values.
    pub column: u32,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at line {}, column {} (byte {})",
            self.message, self.line, self.column, self.offset
        )
    }
}

impl std::error::Error for Error {}

impl Value {
    /// The original number text.
    pub fn as_number(&self) -> Option<&str> {
        match self {
            Value::Number(text) => Some(text),
            _ => None,
        }
    }

    /// `true` or `false`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The string contents.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// The array elements.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Object pairs in stored order.
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(pairs) => Some(pairs),
            _ => None,
        }
    }

    /// The number as `i64` when it is an integer in range.
    pub fn as_i64(&self) -> Option<i64> {
        i64::try_from(integer(self.as_number()?)?).ok()
    }

    /// The number as `u64` when it is an integer in range.
    pub fn as_u64(&self) -> Option<u64> {
        u64::try_from(integer(self.as_number()?)?).ok()
    }

    /// The number as a finite `f64`.
    pub fn as_f64(&self) -> Option<f64> {
        let number: f64 = self.as_number()?.parse().ok()?;
        number.is_finite().then_some(number)
    }

    /// The value of an object name. The last duplicate name is the one stored.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.as_object()?
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// The array element at `index`.
    pub fn get_index(&self, index: usize) -> Option<&Value> {
        self.as_array()?.get(index)
    }
}

/// Integer value of JSON number text, or `None` when it is not an integer.
fn integer(text: &str) -> Option<i128> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if rest.is_empty() {
        return None;
    }
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(index) => (&rest[..index], exponent(&rest[index + 1..])?),
        None => (rest, 0i32),
    };
    let (whole, fraction) = match mantissa.find('.') {
        Some(index) => {
            let fraction = &mantissa[index + 1..];
            if fraction.is_empty() {
                return None;
            }
            (&mantissa[..index], fraction)
        }
        None => (mantissa, ""),
    };
    if !is_digits(whole) || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if whole.len() > 1 && whole.starts_with('0') {
        return None;
    }
    let scale = exponent.checked_sub(fraction.len() as i32)?;
    let mut raw = Vec::with_capacity(whole.len() + fraction.len());
    raw.extend_from_slice(whole.as_bytes());
    raw.extend_from_slice(fraction.as_bytes());
    if scale >= 0 {
        let extra = scale as usize;
        if raw.len().saturating_add(extra) > 39 {
            return None;
        }
        raw.resize(raw.len() + extra, b'0');
    } else {
        let drop = scale.unsigned_abs() as usize;
        if drop > raw.len() {
            return None;
        }
        let keep = raw.len() - drop;
        if raw[keep..].iter().any(|byte| *byte != b'0') {
            return None;
        }
        raw.truncate(keep);
    }
    if raw.iter().all(|byte| *byte == b'0') {
        return Some(0);
    }
    let start = raw
        .iter()
        .position(|byte| *byte != b'0')
        .expect("a nonzero digit");
    let body = std::str::from_utf8(&raw[start..]).expect("ASCII digits");
    let value: i128 = body.parse().ok()?;
    if negative {
        value.checked_neg()
    } else {
        Some(value)
    }
}

fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn exponent(text: &str) -> Option<i32> {
    let (negative, number) = if let Some(rest) = text.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = text.strip_prefix('+') {
        (false, rest)
    } else {
        (false, text)
    };
    if !is_digits(number) {
        return None;
    }
    let value: i32 = number.parse().ok()?;
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests;
