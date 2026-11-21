//! Validation of values against their setting's type.

use std::collections::BTreeSet;

use crate::error::SettingsError;
use crate::schema::{Setting, TextKind, ValueType};
use crate::value::{KeyBinding, Value};

/// Longest key, context or action token in a binding.
pub const MAX_TOKEN_LEN: usize = 32;

/// Checks type, range, step, membership and format. Dependencies are not
/// checked: a disabled setting keeps (and accepts) its value.
pub fn check(setting: &Setting, value: &Value) -> Result<(), SettingsError> {
    let key = setting.key;
    let mismatch = || SettingsError::TypeMismatch {
        key,
        expected: setting.ty.type_name(),
        found: value.type_name(),
    };
    match (&setting.ty, value) {
        (ValueType::Bool { .. }, Value::Bool(_)) => Ok(()),
        (ValueType::Int { min, max, step, .. }, Value::Int(v)) => {
            check_grid(key, *v, *min, *max, *step)
        }
        (
            ValueType::Duration {
                min_ms,
                max_ms,
                step_ms,
                ..
            },
            Value::Duration(v),
        ) => check_grid(key, *v, *min_ms, *max_ms, *step_ms),
        (ValueType::Enum { choices, .. }, Value::Enum(v))
        | (
            ValueType::Color {
                palette: choices, ..
            },
            Value::Color(v),
        ) => {
            if choices.contains(v) {
                Ok(())
            } else {
                Err(SettingsError::UnknownVariant {
                    key,
                    variant: v.clone(),
                })
            }
        }
        (ValueType::Text { kind, max_len, .. }, Value::Text(v)) => {
            check_text(key, *kind, *max_len, v)
        }
        (ValueType::KeyBindings { max_len }, Value::KeyBindings(list)) => {
            check_bindings(key, *max_len, list)
        }
        _ => Err(mismatch()),
    }
}

fn check_grid(
    key: &'static str,
    value: i64,
    min: i64,
    max: i64,
    step: i64,
) -> Result<(), SettingsError> {
    validate_grid(min, max, step).map_err(|_| invalid(key, "invalid numeric grid"))?;
    if value < min || value > max {
        return Err(SettingsError::OutOfRange {
            key,
            value,
            min,
            max,
        });
    }
    if (i128::from(value) - i128::from(min)) % i128::from(step) != 0 {
        return Err(SettingsError::OffStep { key, value, step });
    }
    Ok(())
}

/// A numeric grid must have ordered bounds and a positive step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidGrid;
impl std::fmt::Display for InvalidGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("numeric grid requires min <= max and step > 0")
    }
}
impl std::error::Error for InvalidGrid {}

fn validate_grid(min: i64, max: i64, step: i64) -> Result<(), InvalidGrid> {
    if min > max || step <= 0 {
        Err(InvalidGrid)
    } else {
        Ok(())
    }
}

/// Brings a number into `min..=max` and down onto the step grid.
/// Returns [`InvalidGrid`] for reversed bounds or a nonpositive step.
pub fn clamp_to_grid(value: i64, min: i64, max: i64, step: i64) -> Result<i64, InvalidGrid> {
    clamp_wide_to_grid(i128::from(value), min, max, step)
}

pub(crate) fn clamp_wide_to_grid(
    value: i128,
    min: i64,
    max: i64,
    step: i64,
) -> Result<i64, InvalidGrid> {
    validate_grid(min, max, step)?;
    let v = value.clamp(i128::from(min), i128::from(max));
    let result = i128::from(min) + (v - i128::from(min)) / i128::from(step) * i128::from(step);
    // The validated grid and clamp keep the result in the original i64 bounds.
    Ok(result as i64)
}

fn invalid(key: &'static str, reason: impl Into<String>) -> SettingsError {
    SettingsError::InvalidFormat {
        key,
        reason: reason.into(),
    }
}

fn check_text(
    key: &'static str,
    kind: TextKind,
    max_len: usize,
    text: &str,
) -> Result<(), SettingsError> {
    let len = text.chars().count();
    if len > max_len {
        return Err(SettingsError::TooLong {
            key,
            len,
            max: max_len,
        });
    }
    if text.chars().any(char::is_control) {
        return Err(invalid(key, "contains control characters"));
    }
    match kind {
        TextKind::Plain | TextKind::CommaList | TextKind::Path => Ok(()),
        TextKind::HostPort => {
            if text.is_empty() || is_host_port(text) {
                Ok(())
            } else {
                Err(invalid(key, "expected host:port"))
            }
        }
        TextKind::Locale => {
            if text == "system" || is_language_tag(text) {
                Ok(())
            } else {
                Err(invalid(key, "expected \"system\" or a language tag"))
            }
        }
    }
}

fn is_host_port(text: &str) -> bool {
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.chars().any(char::is_whitespace)
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// `ll`, `lll`, then `-` subtags of 1–8 alphanumerics.
fn is_language_tag(text: &str) -> bool {
    let mut parts = text.split('-');
    let lang = parts.next().unwrap_or_default();
    (2..=3).contains(&lang.len())
        && lang.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// `[a-z][a-z0-9_]*`, at most [`MAX_TOKEN_LEN`] characters.
pub fn is_token(token: &str) -> bool {
    let mut chars = token.chars();
    token.len() <= MAX_TOKEN_LEN
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn check_bindings(
    key: &'static str,
    max_len: usize,
    list: &[KeyBinding],
) -> Result<(), SettingsError> {
    if list.len() > max_len {
        return Err(SettingsError::TooLong {
            key,
            len: list.len(),
            max: max_len,
        });
    }
    let mut seen = BTreeSet::new();
    for b in list {
        let tokens = [b.context.as_deref(), Some(&b.key), Some(&b.action)];
        if let Some(bad) = tokens.into_iter().flatten().find(|t| !is_token(t)) {
            return Err(invalid(key, format!("{bad:?} is not a valid name")));
        }
        if !seen.insert((&b.context, &b.key, b.press)) {
            return Err(invalid(
                key,
                format!("{} {} is bound twice", b.key, b.press.key()),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod grid_regressions {
    use super::*;
    #[test]
    fn invalid_and_full_width_grids_never_panic() {
        for (min, max, step) in [(0, 10, 0), (0, 10, -1), (10, 0, 1), (i64::MIN, i64::MAX, 2)] {
            let valid = min <= max && step > 0;
            assert_eq!(clamp_to_grid(i64::MAX, min, max, step).is_ok(), valid);
            if !valid {
                assert!(check_grid("synthetic", 5, min, max, step).is_err());
            }
        }
        assert_eq!(
            clamp_to_grid(i64::MAX, i64::MIN, i64::MAX, 2),
            Ok(i64::MAX - 1)
        );
        assert_eq!(
            clamp_wide_to_grid(
                i128::from(i64::MIN) + i128::from(i64::MAX) * 2,
                i64::MIN,
                i64::MAX,
                1
            ),
            Ok(i64::MAX - 1)
        );
    }
}
