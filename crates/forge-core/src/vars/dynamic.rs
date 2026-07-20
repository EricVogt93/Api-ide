//! Built-in dynamic variables, e.g. `{{$uuid}}` or `{{$randomInt(1,10)}}`.
//!
//! These are recognised purely by name (all start with `$`) and are
//! re-evaluated on every lookup — two occurrences of `{{$uuid}}` in the
//! same template produce different values.

use base64::prelude::*;
use chrono::{SecondsFormat, Utc};
use rand::distr::Alphanumeric;
use rand::Rng;
use uuid::Uuid;

/// Resolve a built-in dynamic variable by name, including the leading `$`
/// and any `(args)` suffix (e.g. `$randomInt(1,10)`).
///
/// Returns `None` when `name` does not start with `$`, or does not match
/// any known dynamic variable.
pub fn resolve(name: &str) -> Option<String> {
    if !name.starts_with('$') {
        return None;
    }
    let (base, args) = split_args(name);
    match base {
        "$uuid" | "$guid" => Some(Uuid::new_v4().to_string()),
        "$timestamp" => Some(Utc::now().timestamp().to_string()),
        "$timestampMs" => Some(Utc::now().timestamp_millis().to_string()),
        "$isoTimestamp" => Some(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        "$randomInt" => {
            let (min, max) = parse_i64_pair(args).unwrap_or((0, 1000));
            let (min, max) = if min <= max { (min, max) } else { (max, min) };
            Some(rand::rng().random_range(min..=max).to_string())
        }
        "$randomFloat" => {
            let (min, max) = parse_f64_pair(args).unwrap_or((0.0, 1.0));
            let (min, max) = if min <= max { (min, max) } else { (max, min) };
            let unit: f64 = rand::rng().random();
            Some((min + unit * (max - min)).to_string())
        }
        "$randomEmail" => Some(format!("{}@example.com", random_alnum(10).to_lowercase())),
        "$randomString" => {
            let len = parse_usize(args).unwrap_or(16);
            Some(random_alnum(len))
        }
        "$randomHex" => {
            let len = parse_usize(args).unwrap_or(16);
            Some(random_hex(len))
        }
        "$base64" => args.map(|text| BASE64_STANDARD.encode(text.as_bytes())),
        _ => None,
    }
}

/// Split `name` into its base (e.g. `$randomInt`) and, if present, the raw
/// text between a matching trailing `(` … `)` pair.
fn split_args(name: &str) -> (&str, Option<&str>) {
    if let Some(idx) = name.find('(') {
        if name.ends_with(')') {
            return (&name[..idx], Some(&name[idx + 1..name.len() - 1]));
        }
    }
    (name, None)
}

fn parse_i64_pair(args: Option<&str>) -> Option<(i64, i64)> {
    let args = args?;
    let mut parts = args.split(',').map(str::trim);
    let a = parts.next()?.parse::<i64>().ok()?;
    let b = parts.next()?.parse::<i64>().ok()?;
    Some((a, b))
}

fn parse_f64_pair(args: Option<&str>) -> Option<(f64, f64)> {
    let args = args?;
    let mut parts = args.split(',').map(str::trim);
    let a = parts.next()?.parse::<f64>().ok()?;
    let b = parts.next()?.parse::<f64>().ok()?;
    Some((a, b))
}

fn parse_usize(args: Option<&str>) -> Option<usize> {
    args?.trim().parse::<usize>().ok()
}

fn random_alnum(len: usize) -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn random_hex(len: usize) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut rng = rand::rng();
    (0..len)
        .map(|_| HEX[rng.random_range(0..16usize)] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn not_dynamic_returns_none() {
        assert_eq!(resolve("plain"), None);
        assert_eq!(resolve("apiKey"), None);
    }

    #[test]
    fn unknown_dynamic_returns_none() {
        assert_eq!(resolve("$nope"), None);
    }

    #[test]
    fn uuid_and_guid_match_uuid_format() {
        let re =
            Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap();
        assert!(re.is_match(&resolve("$uuid").unwrap()));
        assert!(re.is_match(&resolve("$guid").unwrap()));
    }

    #[test]
    fn uuid_reevaluates_each_call() {
        assert_ne!(resolve("$uuid").unwrap(), resolve("$uuid").unwrap());
    }

    #[test]
    fn timestamp_is_numeric() {
        let ts = resolve("$timestamp").unwrap();
        assert!(ts.parse::<i64>().is_ok());
        let ts_ms = resolve("$timestampMs").unwrap();
        assert!(ts_ms.parse::<i64>().is_ok());
        // ms should be roughly 1000x seconds (same instant, within slack)
        let secs: i64 = ts.parse().unwrap();
        let ms: i64 = ts_ms.parse().unwrap();
        assert!((ms - secs * 1000).abs() < 5000);
    }

    #[test]
    fn iso_timestamp_is_rfc3339_utc() {
        let s = resolve("$isoTimestamp").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&s).is_ok());
        assert!(s.ends_with('Z'));
    }

    #[test]
    fn random_int_default_range() {
        for _ in 0..50 {
            let v: i64 = resolve("$randomInt").unwrap().parse().unwrap();
            assert!((0..=1000).contains(&v));
        }
    }

    #[test]
    fn random_int_with_args() {
        for _ in 0..50 {
            let v: i64 = resolve("$randomInt(5,7)").unwrap().parse().unwrap();
            assert!((5..=7).contains(&v));
        }
    }

    #[test]
    fn random_int_with_reversed_args() {
        let v: i64 = resolve("$randomInt(7,5)").unwrap().parse().unwrap();
        assert!((5..=7).contains(&v));
    }

    #[test]
    fn random_float_default_range() {
        for _ in 0..50 {
            let v: f64 = resolve("$randomFloat").unwrap().parse().unwrap();
            assert!((0.0..1.0).contains(&v));
        }
    }

    #[test]
    fn random_float_with_args() {
        let v: f64 = resolve("$randomFloat(10,20)").unwrap().parse().unwrap();
        assert!((10.0..=20.0).contains(&v));
    }

    #[test]
    fn random_email_format() {
        let re = Regex::new(r"^[a-z0-9]+@example\.com$").unwrap();
        let e = resolve("$randomEmail").unwrap();
        assert!(re.is_match(&e), "{e} did not match");
    }

    #[test]
    fn random_string_default_len() {
        let s = resolve("$randomString").unwrap();
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn random_string_with_len() {
        let s = resolve("$randomString(5)").unwrap();
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn random_hex_default_len() {
        let s = resolve("$randomHex").unwrap();
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn random_hex_with_len() {
        let s = resolve("$randomHex(4)").unwrap();
        assert_eq!(s.len(), 4);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn base64_encodes_literal_arg() {
        assert_eq!(resolve("$base64(hello)").unwrap(), "aGVsbG8=");
    }

    #[test]
    fn base64_without_args_is_none() {
        assert_eq!(resolve("$base64"), None);
    }
}
