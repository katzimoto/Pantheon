//! Content digests for configuration identity.

use std::fmt;

use sha2::{Digest as _, Sha256};

/// A SHA-256 content digest.
///
/// `docs/architecture/operations/configuration-and-policy-revisions.md`
/// ("Canonical hashing") makes digests configuration *identity*: an immutable
/// decision records the exact component digest that affected it. The SQLite
/// operating rules require 32-byte SHA-256 digests stored as BLOBs where a
/// digest is a relational field, which is what the 32-byte representation here
/// is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest([u8; 32]);

impl Digest {
    /// Digests `bytes`.
    ///
    /// Callers pass canonical bytes — see [`crate::config::canonical`]. This
    /// function does not canonicalize anything, because a digest over
    /// non-canonical input is not an identity.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// The raw 32 bytes, for storage as a BLOB.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuilds a digest read back from storage.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parses the stable `sha256:<hex>` display form.
    #[must_use]
    pub fn from_display(text: &str) -> Option<Self> {
        let hex = text.strip_prefix("sha256:")?;
        if hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = hex.as_bytes()[index * 2];
            let low = hex.as_bytes()[index * 2 + 1];
            *byte = (hex_nibble(high)? << 4) | hex_nibble(low)?;
        }
        Some(Self(bytes))
    }

    /// The lowercase hexadecimal form, without the algorithm prefix.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            // Deliberately not `format!("{:02x}")` in a loop: this is on the
            // digest path for every component of every candidate.
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Digest {
    /// Renders as `sha256:<hex>`, the form the configuration contract uses.
    ///
    /// The algorithm is part of the rendered identity so a future digest change
    /// is visible rather than silently reinterpreted.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

#[cfg(test)]
mod tests;
