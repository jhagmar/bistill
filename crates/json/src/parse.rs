use crate::{Error, MAX_BYTES, MAX_DEPTH, Value};

/// Parse UTF-8 JSON bytes into a [`Value`](crate::Value).
///
/// ```
/// let value = json::parse(br#"{"n":1e2}"#).unwrap();
/// assert_eq!(value.get("n").and_then(json::Value::as_i64), Some(100));
/// ```
pub fn parse(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > MAX_BYTES {
        return Err(Error {
            message: "body exceeds 8 MiB".to_owned(),
            offset: 0,
            line: 1,
            column: 1,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|err| {
        let offset = err.valid_up_to();
        let (line, column) = line_column(bytes, offset);
        Error {
            message: "input is not UTF-8".to_owned(),
            offset,
            line,
            column,
        }
    })?;
    let mut parser = Parser {
        text,
        index: 0,
        line: 1,
        column: 1,
        depth: 0,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.index != parser.text.len() {
        return Err(parser.error("trailing data"));
    }
    Ok(value)
}

fn line_column(bytes: &[u8], offset: usize) -> (u32, u32) {
    let text = std::str::from_utf8(&bytes[..offset]).expect("valid UTF-8 prefix");
    let mut line = 1u32;
    let mut column = 1u32;
    for ch in text.chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

struct Parser<'a> {
    text: &'a str,
    index: usize,
    line: u32,
    column: u32,
    depth: u32,
}

impl Parser<'_> {
    fn value(&mut self) -> Result<Value, Error> {
        let Some(ch) = self.peek() else {
            return Err(self.error("expected a value"));
        };
        if ch == 'n' && self.keyword("null") {
            return Ok(Value::Null);
        }
        if ch == 't' && self.keyword("true") {
            return Ok(Value::Bool(true));
        }
        if ch == 'f' && self.keyword("false") {
            return Ok(Value::Bool(false));
        }
        if ch == '"' {
            return Ok(Value::String(self.string()?));
        }
        if ch == '[' {
            return self.array();
        }
        if ch == '{' {
            return self.object();
        }
        if ch == '-' || ch.is_ascii_digit() {
            return self.number();
        }
        Err(self.error("expected a value"))
    }

    fn keyword(&mut self, word: &str) -> bool {
        let rest = &self.text[self.index..];
        if !rest.starts_with(word) {
            return false;
        }
        let ident = rest[word.len()..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if ident {
            return false;
        }
        for _ in word.chars() {
            self.bump();
        }
        true
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.bump();
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => self.bump(),
                Some(']') => {
                    self.bump();
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
            self.skip_ws();
        }
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.bump();
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            self.depth -= 1;
            return Ok(Value::Object(pairs));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err(self.error("expected a string key"));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(':') {
                return Err(self.error("expected ':'"));
            }
            self.bump();
            self.skip_ws();
            let value = self.value()?;
            if let Some(pos) = pairs.iter().position(|(name, _)| name == &key) {
                pairs.remove(pos);
            }
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(',') => self.bump(),
                Some('}') => {
                    self.bump();
                    self.depth -= 1;
                    return Ok(Value::Object(pairs));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error("nesting exceeds 128"));
        }
        Ok(())
    }

    fn string(&mut self) -> Result<String, Error> {
        self.bump();
        let mut out = String::new();
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            if ch == '"' {
                self.bump();
                return Ok(out);
            }
            if ch == '\\' {
                self.bump();
                out.push(self.escape()?);
                continue;
            }
            if (ch as u32) < 0x20 {
                return Err(self.error("unescaped control character"));
            }
            self.bump();
            out.push(ch);
        }
    }

    fn escape(&mut self) -> Result<char, Error> {
        let Some(ch) = self.peek() else {
            return Err(self.error("truncated escape"));
        };
        self.bump();
        match ch {
            '"' | '\\' | '/' => Ok(ch),
            'b' => Ok('\u{0008}'),
            'f' => Ok('\u{000c}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'u' => self.unicode(),
            _ => Err(self.error("invalid escape")),
        }
    }

    fn unicode(&mut self) -> Result<char, Error> {
        let unit = self.hex4()?;
        if (0xD800..0xDC00).contains(&unit) {
            if self.peek() != Some('\\') {
                return Err(self.error("lone surrogate"));
            }
            self.bump();
            if self.peek() != Some('u') {
                return Err(self.error("lone surrogate"));
            }
            self.bump();
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(self.error("invalid surrogate pair"));
            }
            let code = 0x10000 + (((unit as u32 - 0xD800) << 10) | (low as u32 - 0xDC00));
            return Ok(char::from_u32(code).expect("surrogate pair is a scalar"));
        }
        if (0xDC00..0xE000).contains(&unit) {
            return Err(self.error("lone surrogate"));
        }
        Ok(char::from_u32(unit as u32).expect("non-surrogate is a scalar"))
    }

    fn hex4(&mut self) -> Result<u16, Error> {
        let mut value = 0u16;
        for _ in 0..4 {
            let Some(ch) = self.peek() else {
                return Err(self.error("truncated unicode escape"));
            };
            let Some(digit) = ch.to_digit(16) else {
                return Err(self.error("invalid unicode escape"));
            };
            self.bump();
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.index;
        if self.peek() == Some('-') {
            let digit = self.text[self.index + 1..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit());
            if !digit {
                return Err(self.error("invalid number"));
            }
            self.bump();
        }
        if self.peek() == Some('0') {
            self.bump();
        } else {
            while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                self.bump();
            }
        }
        if self.peek() == Some('.') {
            self.bump();
            if !self.digits() {
                return Err(self.error("invalid number"));
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if !self.digits() {
                return Err(self.error("invalid number"));
            }
        }
        Ok(Value::Number(self.text[start..self.index].to_owned()))
    }

    fn digits(&mut self) -> bool {
        if !self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            return false;
        }
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.bump();
        }
        true
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.index..].chars().next()
    }

    fn bump(&mut self) {
        let ch = self.peek().expect("a character");
        self.index += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
    }

    fn error(&self, message: &str) -> Error {
        Error {
            message: message.to_owned(),
            offset: self.index,
            line: self.line,
            column: self.column,
        }
    }
}
