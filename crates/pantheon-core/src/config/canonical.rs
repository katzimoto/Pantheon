//! The canonical value form configuration is digested from.
//!
//! Issue #23 requires that semantically identical configuration produce
//! identical identities, and that identity never depend on map iteration
//! order, debug formatting, source ordering that carries no meaning, or
//! unstable serializer behaviour. This module is where that is made true
//! rather than hoped for.
//!
//! Three decisions carry that weight:
//!
//! - **Objects are [`BTreeMap`].** Key order is a property of the type, not of
//!   an insertion sequence or a hash seed, so two runs cannot disagree.
//! - **There are no floating-point values.** Configuration has no need for
//!   them, and their textual form is the classic source of digests that differ
//!   between platforms or library versions. Admitting them would be admitting
//!   nondeterminism for no gain.
//! - **Encoding is total and explicit.** [`Value::to_canonical_bytes`] writes
//!   the bytes itself instead of delegating to a serializer whose formatting is
//!   outside Pantheon's control.
//!
//! The encoding is a strict JSON subset: no insignificant whitespace, object
//! keys in sorted order, and one fixed escape form per character that needs
//! escaping.

use std::collections::BTreeMap;
use std::fmt;

use crate::config::Digest;

/// A canonical configuration value.
///
/// This is the whole value domain configuration may express. It is
/// deliberately small: every constructor of a compiled component lowers into
/// this, so anything expressible here is something a digest must be able to
/// reproduce byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Value {
    Null,
    Bool(bool),
    /// A 64-bit signed integer. Configuration uses integral quantities only —
    /// see the module documentation on why floats are excluded.
    Integer(i64),
    String(String),
    Array(Vec<Value>),
    /// Sorted by key, by construction.
    Object(BTreeMap<String, Value>),
}

impl Value {
    /// Builds an object from pairs, without the caller needing to name
    /// [`BTreeMap`] or worry about insertion order.
    #[must_use]
    pub fn object<I, K>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, Self)>,
        K: Into<String>,
    {
        Self::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    /// Builds a string value.
    #[must_use]
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    /// Builds an array value.
    #[must_use]
    pub fn array<I: IntoIterator<Item = Self>>(values: I) -> Self {
        Self::Array(values.into_iter().collect())
    }

    /// The canonical byte encoding of this value.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write_canonical(&mut out);
        out.into_bytes()
    }

    /// The content digest of this value's canonical encoding.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::of(&self.to_canonical_bytes())
    }

    fn write_canonical(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            // Integer formatting in Rust is exact and platform-independent,
            // unlike float formatting — which is why the value domain has no
            // floats.
            Self::Integer(value) => out.push_str(&value.to_string()),
            Self::String(value) => write_canonical_string(value, out),
            Self::Array(values) => {
                out.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    value.write_canonical(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                // `BTreeMap` iterates in key order, so the sort is structural
                // rather than a step that could be forgotten.
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_canonical_string(key, out);
                    out.push(':');
                    value.write_canonical(out);
                }
                out.push('}');
            }
        }
    }

    /// Looks up a key on an object value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries.get(key),
            _ => None,
        }
    }

    /// The name of this value's kind, for diagnostics.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }
}

impl fmt::Display for Value {
    /// The canonical encoding. There is deliberately only one rendering of a
    /// value, so a diagnostic and a digest can never disagree about what the
    /// configuration says.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        self.write_canonical(&mut out);
        f.write_str(&out)
    }
}

/// Writes a JSON string with exactly one escape form per escaped character.
///
/// Encoders differ on optional escaping — `/`, non-ASCII, and which control
/// characters get short forms. Fixing one form here is what keeps the digest
/// stable across any future change to how Pantheon builds these values.
fn write_canonical_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Remaining C0 controls have no short form; use the fixed
            // lowercase four-digit escape.
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                let code = c as u32;
                for shift in [12, 8, 4, 0] {
                    let nibble = (code >> shift) & 0xf;
                    out.push(char::from_digit(nibble, 16).unwrap_or('0'));
                }
            }
            // Everything else is emitted literally, including non-ASCII: the
            // encoding is UTF-8, so escaping it would only add a second way to
            // spell the same string.
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests;
