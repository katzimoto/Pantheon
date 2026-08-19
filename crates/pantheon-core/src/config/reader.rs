//! Typed field access over the canonical value domain.
//!
//! Split out of [`crate::config::compile`](mod@crate::config::compile) so the compilation pipeline reads
//! as the pipeline the contract describes, rather than as pipeline steps
//! interleaved with accessor plumbing. Every accessor reports the field path
//! it failed at, because an operator fixing a rejected candidate needs to know
//! where, not just that.

use crate::config::canonical::Value;
use crate::config::error::ConfigError;

pub(crate) fn path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

pub(crate) fn field<'a>(
    value: &'a Value,
    prefix: &str,
    key: &str,
) -> Result<&'a Value, ConfigError> {
    value.get(key).ok_or_else(|| ConfigError::MissingField {
        path: path(prefix, key),
    })
}

pub(crate) fn as_str<'a>(value: &'a Value, at: &str) -> Result<&'a str, ConfigError> {
    match value {
        Value::String(text) => Ok(text),
        other => Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: format!("expected a string, found {}", other.kind()),
        }),
    }
}

pub(crate) fn as_i64(value: &Value, at: &str) -> Result<i64, ConfigError> {
    match value {
        Value::Integer(number) => Ok(*number),
        other => Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: format!("expected an integer, found {}", other.kind()),
        }),
    }
}

pub(crate) fn as_bool(value: &Value, at: &str) -> Result<bool, ConfigError> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        other => Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: format!("expected a boolean, found {}", other.kind()),
        }),
    }
}

pub(crate) fn as_array<'a>(value: &'a Value, at: &str) -> Result<&'a [Value], ConfigError> {
    match value {
        Value::Array(values) => Ok(values),
        other => Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: format!("expected an array, found {}", other.kind()),
        }),
    }
}

pub(crate) fn string_list(
    parent: &Value,
    prefix: &str,
    key: &str,
) -> Result<Vec<String>, ConfigError> {
    let at = path(prefix, key);
    as_array(field(parent, prefix, key)?, &at)?
        .iter()
        .map(|entry| as_str(entry, &at).map(ToString::to_string))
        .collect()
}

pub(crate) fn non_empty(text: &str, at: &str) -> Result<(), ConfigError> {
    if text.is_empty() {
        return Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: "must not be empty".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn positive(number: i64, at: &str) -> Result<(), ConfigError> {
    if number <= 0 {
        return Err(ConfigError::InvalidValue {
            path: at.to_string(),
            detail: format!("must be greater than zero, found {number}"),
        });
    }
    Ok(())
}

/// Rejects a repeated identity, which is a conflicting declaration rather
/// than a redefinition.
pub(crate) fn unique(
    seen: &mut Vec<String>,
    kind: &'static str,
    id: &str,
) -> Result<(), ConfigError> {
    if seen.iter().any(|existing| existing == id) {
        return Err(ConfigError::DuplicateIdentity {
            kind,
            id: id.to_string(),
        });
    }
    seen.push(id.to_string());
    Ok(())
}
