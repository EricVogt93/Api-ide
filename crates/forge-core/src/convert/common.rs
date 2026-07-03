//! Small helpers shared between curl export and code-snippet generation.

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::model::{ParamKind, RequestDef};

/// RFC 3986 unreserved characters stay unescaped; everything else is
/// percent-encoded. Matches curl's `--data-urlencode` behaviour.
const FORM_ENCODE_SET: &AsciiSet =
    &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// Percent-encode a string the way curl's `--data-urlencode` and
/// `application/x-www-form-urlencoded` bodies do.
pub(crate) fn percent_encode_form(s: &str) -> String {
    utf8_percent_encode(s, FORM_ENCODE_SET).to_string()
}

/// Enabled, non-empty query-string params on a request (path params are
/// left out — they belong in the URL template, not the query string).
pub(crate) fn query_pairs(def: &RequestDef) -> Vec<(String, String)> {
    def.params
        .iter()
        .filter(|p| p.kv.is_active() && p.kind == ParamKind::Query)
        .map(|p| (p.kv.key.clone(), p.kv.value.clone()))
        .collect()
}

/// Append `pairs` to `url`'s query string, joining with `&`/`?` as needed.
/// Values are kept verbatim (no percent-encoding) so `{{variables}}` survive.
pub(crate) fn append_query(url: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return url.to_string();
    }
    let mut out = url.to_string();
    let mut first = !out.contains('?');
    for (k, v) in pairs {
        out.push(if first { '?' } else { '&' });
        first = false;
        out.push_str(k);
        out.push('=');
        out.push_str(v);
    }
    out
}

/// Enabled headers as plain string pairs.
pub(crate) fn enabled_headers(def: &RequestDef) -> Vec<(String, String)> {
    def.headers
        .iter()
        .filter(|h| h.is_active())
        .map(|h| (h.key.clone(), h.value.clone()))
        .collect()
}

pub(crate) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

pub(crate) fn has_header(headers: &[(String, String)], name: &str) -> bool {
    header_value(headers, name).is_some()
}

/// Build the compact JSON body curl / http clients send for a GraphQL
/// request: `{"query": ..., "variables": {...}, "operationName": ...}`.
pub(crate) fn graphql_json_body(
    query: &str,
    variables: &str,
    operation_name: &Option<String>,
) -> String {
    let vars: serde_json::Value = if variables.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(variables).unwrap_or_else(|_| serde_json::json!({}))
    };
    let mut obj = serde_json::Map::new();
    obj.insert("query".to_string(), serde_json::Value::String(query.to_string()));
    obj.insert("variables".to_string(), vars);
    if let Some(op) = operation_name {
        obj.insert("operationName".to_string(), serde_json::Value::String(op.clone()));
    }
    serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_default()
}

/// Quote a string as a single-quoted POSIX shell argument, escaping any
/// embedded single quotes with the classic `'"'"'` technique.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'"'"'"#))
}
