//! The strict source parser.
//!
//! Configuration source is JSON. Pantheon parses it here rather than through a
//! general-purpose deserializer because Issue #23 makes the parse a *semantic*
//! boundary: malformed source must be rejected with a typed failure, and the
//! parsed value must be the canonical value domain whose encoding defines
//! configuration identity. A permissive parser would let two different source
//! texts mean the same thing in ways Pantheon never decided.
//!
//! The accepted grammar is deliberately narrower than JSON:
//!
//! - **no floating-point numbers** — the canonical value domain has none, so
//!   accepting them would create values that cannot be digested;
//! - **no duplicate object keys** — a duplicate is a conflicting declaration,
//!   not a last-one-wins merge;
//! - **no comments and no trailing commas** — neither is JSON, and accepting
//!   them would make Pantheon's dialect a thing operators must learn.

use std::collections::BTreeMap;
use std::fmt;

use crate::config::canonical::Value;

/// A source text that is not valid Pantheon configuration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset where the problem was detected.
    pub offset: usize,
    pub detail: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "malformed configuration source at byte {}: {}",
            self.offset, self.detail
        )
    }
}

impl std::error::Error for ParseError {}

/// Parses configuration source text into the canonical value domain.
///
/// # Errors
///
/// [`ParseError`] when the text is not a single well-formed value in the
/// accepted grammar.
pub fn parse(source: &str) -> Result<Value, ParseError> {
    let mut parser = Parser {
        bytes: source.as_bytes(),
        offset: 0,
    };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.offset != parser.bytes.len() {
        return Err(parser.error("trailing content after the top-level value"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Parser<'_> {
    fn error(&self, detail: impl Into<String>) -> ParseError {
        ParseError {
            offset: self.offset,
            detail: detail.into(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.offset += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.offset += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected {:?}", char::from(byte))))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, ParseError> {
        if self.bytes[self.offset..].starts_with(word.as_bytes()) {
            self.offset += word.len();
            Ok(value)
        } else {
            Err(self.error(format!("expected {word}")))
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            None => Err(self.error("unexpected end of source")),
            Some(b'n') => self.literal("null", Value::Null),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(b'-' | b'0'..=b'9') => self.integer(),
            Some(byte) => Err(self.error(format!("unexpected byte {:?}", char::from(byte)))),
        }
    }

    fn integer(&mut self) -> Result<Value, ParseError> {
        let start = self.offset;
        if self.peek() == Some(b'-') {
            self.offset += 1;
        }
        let digits_start = self.offset;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.offset += 1;
        }
        if self.offset == digits_start {
            return Err(self.error("expected a digit"));
        }
        // Reject anything that would have been a float. Silently truncating
        // `1.5` to `1` would make two different source texts compile to the
        // same configuration.
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(
                self.error("fractional and exponent numbers are not valid configuration values")
            );
        }
        let text = std::str::from_utf8(&self.bytes[start..self.offset])
            .map_err(|_| self.error("invalid utf-8 in number"))?;
        // Leading zeros are rejected: `01` and `1` would otherwise be two
        // spellings of one value.
        let unsigned = text.strip_prefix('-').unwrap_or(text);
        if unsigned.len() > 1 && unsigned.starts_with('0') {
            return Err(self.error("numbers may not have leading zeros"));
        }
        text.parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| self.error("integer out of range"))
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    self.offset += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.offset += 1;
                    let Some(escape) = self.peek() else {
                        return Err(self.error("unterminated escape"));
                    };
                    self.offset += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => out.push(self.unicode_escape()?),
                        other => {
                            return Err(
                                self.error(format!("unknown escape {:?}", char::from(other)))
                            );
                        }
                    }
                }
                b if b < 0x20 => {
                    return Err(self.error("unescaped control character in string"));
                }
                _ => {
                    // Step over one whole UTF-8 character.
                    let rest = std::str::from_utf8(&self.bytes[self.offset..])
                        .map_err(|_| self.error("invalid utf-8 in string"))?;
                    let ch = rest.chars().next().unwrap_or('\u{fffd}');
                    out.push(ch);
                    self.offset += ch.len_utf8();
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        if self.offset + 4 > self.bytes.len() {
            return Err(self.error("truncated \\u escape"));
        }
        let hex = std::str::from_utf8(&self.bytes[self.offset..self.offset + 4])
            .map_err(|_| self.error("invalid utf-8 in \\u escape"))?;
        let code = u32::from_str_radix(hex, 16).map_err(|_| self.error("invalid \\u escape"))?;
        self.offset += 4;
        // Surrogate halves are rejected rather than paired: configuration has
        // no need for them and accepting them adds a second spelling of
        // characters that can be written directly.
        char::from_u32(code).ok_or_else(|| self.error("\\u escape is not a scalar value"))
    }

    fn array(&mut self) -> Result<Value, ParseError> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.offset += 1;
            return Ok(Value::Array(values));
        }
        loop {
            self.skip_whitespace();
            values.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b']') => {
                    self.offset += 1;
                    return Ok(Value::Array(values));
                }
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn object(&mut self) -> Result<Value, ParseError> {
        self.expect(b'{')?;
        let mut entries = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.offset += 1;
            return Ok(Value::Object(entries));
        }
        loop {
            self.skip_whitespace();
            let key_offset = self.offset;
            let key = self.string()?;
            if entries.contains_key(&key) {
                return Err(ParseError {
                    offset: key_offset,
                    detail: format!("duplicate key {key:?}"),
                });
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.value()?;
            entries.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.offset += 1,
                Some(b'}') => {
                    self.offset += 1;
                    return Ok(Value::Object(entries));
                }
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }
}

#[cfg(test)]
mod tests;
