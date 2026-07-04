//! Parse a curl command line into a [`RequestDef`].

use crate::convert::common::percent_encode_form;
use crate::model::{
    AuthConfig, BodyDef, KeyValue, Method, MultipartPart, PartContent, RawLanguage, RequestDef,
    RequestSettings,
};

/// Errors that can occur while parsing a curl command line.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CurlParseError {
    #[error("could not tokenize the command line (unbalanced quotes?)")]
    Tokenize,
    #[error("flag {0} requires a value")]
    MissingValue(String),
    #[error("no URL found in the command")]
    MissingUrl,
    #[error("unknown HTTP method: {0}")]
    UnknownMethod(String),
    #[error("invalid -F/--form value: {0}")]
    InvalidForm(String),
}

/// Short curl flags that take a value, whether recognized explicitly by
/// this parser or not. Used to split attached forms (`-XPOST`) and to
/// decide, for flags we don't otherwise implement, whether to also skip
/// their value token.
const VALUE_SHORT_FLAGS: &[char] = &[
    'A', 'b', 'c', 'd', 'D', 'e', 'E', 'F', 'H', 'K', 'm', 'o', 'r', 'T', 'u', 'U', 'w', 'x', 'X', 'C', 't', 'z', 'Y',
    'y', 'P', 'Q',
];

/// Long curl flags (beyond the ones this parser implements) that are known
/// to take a value. Used purely as a heuristic so an unrecognized flag's
/// value isn't mistaken for the URL positional argument.
const VALUE_LONG_FLAGS: &[&str] = &[
    "--max-time",
    "--connect-timeout",
    "--retry",
    "--retry-delay",
    "--retry-max-time",
    "--cacert",
    "--capath",
    "--cert",
    "--cert-type",
    "--key",
    "--key-type",
    "--resolve",
    "--connect-to",
    "--interface",
    "--limit-rate",
    "--range",
    "--write-out",
    "--output",
    "--cookie-jar",
    "--unix-socket",
    "--upload-file",
    "--proxy-user",
    "--dns-servers",
    "--pinnedpubkey",
    "--config",
    "--continue-at",
    "--telnet-option",
    "--form-string",
    "--http1.0",
    "--proxy-cacert",
    "--time-cond",
    "--ciphers",
    "--keepalive-time",
    "--local-port",
    "--max-filesize",
];

/// Split a curl short-option cluster / attached-value token
/// (`-XPOST`, `-sSL`, `--header=foo`) into separate flag/value tokens.
fn normalize_tokens(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tokens.len());
    for tok in tokens {
        if let Some(rest) = tok.strip_prefix("--") {
            if let Some(eq) = rest.find('=') {
                out.push(format!("--{}", &rest[..eq]));
                out.push(rest[eq + 1..].to_string());
            } else {
                out.push(tok);
            }
        } else if tok.starts_with('-') && tok.len() > 2 {
            let rest = &tok[1..];
            let bytes: Vec<char> = rest.chars().collect();
            let mut i = 0;
            while i < bytes.len() {
                let c = bytes[i];
                out.push(format!("-{c}"));
                if VALUE_SHORT_FLAGS.contains(&c) {
                    if i + 1 < bytes.len() {
                        let value: String = bytes[i + 1..].iter().collect();
                        out.push(value);
                    }
                    break;
                }
                i += 1;
            }
        } else {
            out.push(tok);
        }
    }
    out
}

fn parse_header(raw: &str) -> KeyValue {
    if let Some(idx) = raw.find(':') {
        KeyValue::new(raw[..idx].trim(), raw[idx + 1..].trim())
    } else if let Some(name) = raw.strip_suffix(';') {
        KeyValue::new(name.trim(), "")
    } else {
        KeyValue::new(raw.trim(), "")
    }
}

fn parse_form_part(raw: &str) -> Result<MultipartPart, CurlParseError> {
    let (name, rest) = raw
        .split_once('=')
        .ok_or_else(|| CurlParseError::InvalidForm(raw.to_string()))?;
    let (value_part, content_type) = match rest.split_once(";type=") {
        Some((v, ct)) => (v, Some(ct.to_string())),
        None => (rest, None),
    };
    let content = match value_part.strip_prefix('@') {
        Some(path) => PartContent::File { path: path.to_string() },
        None => PartContent::Text { value: value_part.to_string() },
    };
    Ok(MultipartPart { name: name.to_string(), content, content_type, enabled: true })
}

/// Encode one `--data-urlencode` directive per curl's own rules:
/// `content`, `=content`, `name=content`, `name=@file`, `@file`.
fn urlencode_data_directive(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix('=') {
        return percent_encode_form(rest);
    }
    if let Some((name, val)) = raw.split_once('=') {
        if let Some(file) = val.strip_prefix('@') {
            return format!("{name}=@{file}");
        }
        return format!("{name}={}", percent_encode_form(val));
    }
    if let Some(file) = raw.strip_prefix('@') {
        return format!("@{file}");
    }
    percent_encode_form(raw)
}

/// If a raw `-d`/`--data*`/`--data-urlencode` directive references an
/// on-disk file (curl's `@file` / `name=@file` convention), return a note
/// explaining that the file's contents were not imported — otherwise the
/// literal `@file` text would silently end up embedded in the body.
fn data_file_note(raw: &str) -> Option<String> {
    let file = match raw.strip_prefix('@') {
        Some(f) => f,
        None => raw.split_once('=')?.1.strip_prefix('@')?,
    };
    Some(format!("body references file @{file}; file contents were not imported"))
}

/// Strip scheme/query/fragment down to `host/path`, tolerating
/// `{{variable}}` templates that `url::Url` would reject.
fn host_and_path(url: &str) -> String {
    let without_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let end = without_scheme
        .find(['?', '#'])
        .unwrap_or(without_scheme.len());
    without_scheme[..end].to_string()
}

/// Parse a curl command line (as copied from a browser's devtools, Postman,
/// or typed by hand) into a [`RequestDef`].
pub fn parse_curl(cmd: &str) -> Result<RequestDef, CurlParseError> {
    let cleaned = cmd.replace("\\\r\n", " ").replace("\\\n", " ");
    let raw_tokens = shlex::split(&cleaned).ok_or(CurlParseError::Tokenize)?;
    let tokens = normalize_tokens(raw_tokens);
    let mut iter = tokens.into_iter().peekable();

    if matches!(iter.peek(), Some(t) if t.eq_ignore_ascii_case("curl")) {
        iter.next();
    }

    let mut explicit_method: Option<Method> = None;
    let mut head_flag = false;
    let mut get_flag = false;
    let mut positional_urls: Vec<String> = Vec::new();
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data_segments: Vec<String> = Vec::new();
    let mut form_parts: Vec<MultipartPart> = Vec::new();
    let mut basic_auth: Option<(String, String)> = None;
    let mut cookie: Option<String> = None;
    let mut follow_redirects = false;
    let mut insecure = false;
    let mut compressed = false;
    let mut user_agent: Option<String> = None;
    let mut referer: Option<String> = None;
    let mut notes: Vec<String> = Vec::new();

    let take_value = |iter: &mut std::iter::Peekable<std::vec::IntoIter<String>>,
                       flag: &str|
     -> Result<String, CurlParseError> {
        iter.next().ok_or_else(|| CurlParseError::MissingValue(flag.to_string()))
    };

    while let Some(tok) = iter.next() {
        match tok.as_str() {
            "-X" | "--request" => {
                let v = take_value(&mut iter, &tok)?;
                explicit_method =
                    Some(Method::parse(&v).ok_or_else(|| CurlParseError::UnknownMethod(v.clone()))?);
            }
            "-H" | "--header" => {
                let v = take_value(&mut iter, &tok)?;
                headers.push(parse_header(&v));
            }
            "-d" | "--data" | "--data-ascii" | "--data-binary" => {
                let v = take_value(&mut iter, &tok)?;
                if let Some(note) = data_file_note(&v) {
                    notes.push(note);
                }
                data_segments.push(v);
            }
            "--data-raw" => {
                // `--data-raw` takes the value as literal text; unlike
                // `-d`/`--data`, curl does not resolve a leading `@` to a
                // file for it, so no file-import note is warranted here.
                let v = take_value(&mut iter, &tok)?;
                data_segments.push(v);
            }
            "--data-urlencode" => {
                let v = take_value(&mut iter, &tok)?;
                if let Some(note) = data_file_note(&v) {
                    notes.push(note);
                }
                data_segments.push(urlencode_data_directive(&v));
            }
            "-F" | "--form" => {
                let v = take_value(&mut iter, &tok)?;
                form_parts.push(parse_form_part(&v)?);
            }
            "-u" | "--user" => {
                let v = take_value(&mut iter, &tok)?;
                let (user, pass) = v.split_once(':').unwrap_or((v.as_str(), ""));
                basic_auth = Some((user.to_string(), pass.to_string()));
            }
            "-b" | "--cookie" => {
                let v = take_value(&mut iter, &tok)?;
                cookie = Some(v);
            }
            "-L" | "--location" => follow_redirects = true,
            "-k" | "--insecure" => insecure = true,
            "-x" | "--proxy" => {
                let v = take_value(&mut iter, &tok)?;
                notes.push(format!("proxy '{v}' from the imported curl command was not applied"));
            }
            "--compressed" => compressed = true,
            "-A" | "--user-agent" => {
                let v = take_value(&mut iter, &tok)?;
                user_agent = Some(v);
            }
            "-e" | "--referer" => {
                let v = take_value(&mut iter, &tok)?;
                referer = Some(v);
            }
            "-I" | "--head" => head_flag = true,
            "-G" | "--get" => get_flag = true,
            "--url" => {
                let v = take_value(&mut iter, &tok)?;
                positional_urls.push(v);
            }
            "-o" | "--output" | "-w" | "--write-out" => {
                iter.next();
            }
            "-s" | "--silent" | "-v" | "--verbose" | "-i" | "--include" | "-S" | "--show-error"
            | "-f" | "--fail" | "-g" | "--globoff" | "-N" | "--no-buffer" | "-O"
            | "--remote-name" | "-q" | "-#" | "--progress-bar" => {
                // boolean flags we don't act on
            }
            other => {
                if let Some(short) = other.strip_prefix('-').filter(|s| !s.starts_with('-')) {
                    if short.chars().next().is_some_and(|c| VALUE_SHORT_FLAGS.contains(&c)) {
                        iter.next();
                    }
                } else if other.starts_with("--") {
                    if VALUE_LONG_FLAGS.contains(&other) {
                        iter.next();
                    }
                } else {
                    positional_urls.push(other.to_string());
                }
            }
        }
    }

    // Prefer a token that looks like an absolute URL (has a scheme); this
    // guards against a value we failed to recognize as belonging to some
    // flag being mistaken for the URL when a real URL is also present.
    let mut url = positional_urls
        .iter()
        .find(|u| u.contains("://"))
        .cloned()
        .or_else(|| positional_urls.into_iter().next())
        .ok_or(CurlParseError::MissingUrl)?;
    if !url.contains("://") {
        url = format!("https://{url}");
    }

    let method = if let Some(m) = explicit_method {
        m
    } else if get_flag {
        Method::Get
    } else if head_flag {
        Method::Head
    } else if !data_segments.is_empty() || !form_parts.is_empty() {
        Method::Post
    } else {
        Method::Get
    };

    if get_flag && !data_segments.is_empty() {
        let joined = data_segments.join("&");
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&joined);
        data_segments.clear();
    }

    if let Some(c) = cookie {
        headers.push(KeyValue::new("Cookie", c));
    }
    if compressed {
        headers.push(KeyValue::new("Accept-Encoding", "gzip, deflate, br"));
    }
    if let Some(ua) = user_agent {
        headers.push(KeyValue::new("User-Agent", ua));
    }
    if let Some(r) = referer {
        headers.push(KeyValue::new("Referer", r));
    }

    let content_type_is_json = headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .is_some_and(|h| h.value.to_ascii_lowercase().contains("application/json"));

    let body = if !form_parts.is_empty() {
        BodyDef::Multipart { parts: form_parts }
    } else if !data_segments.is_empty() {
        let text = data_segments.join("&");
        if content_type_is_json {
            BodyDef::Json { text }
        } else {
            let language = if serde_json::from_str::<serde_json::Value>(&text).is_ok() {
                RawLanguage::Json
            } else {
                RawLanguage::Text
            };
            BodyDef::Raw { text, language }
        }
    } else {
        BodyDef::None
    };

    let auth = match basic_auth {
        Some((username, password)) => AuthConfig::Basic { username, password },
        None => AuthConfig::Inherit,
    };
    let settings = RequestSettings {
        timeout_ms: None,
        follow_redirects: if follow_redirects { Some(true) } else { None },
        max_redirects: None,
        verify_tls: if insecure { Some(false) } else { None },
        skip_in_runs: false,
    };

    let name = format!("{} {}", method.as_str(), host_and_path(&url));

    Ok(RequestDef {
        format: crate::FORMAT_VERSION,
        name,
        description: notes.join("\n"),
        method,
        url,
        params: Vec::new(),
        headers,
        auth,
        body,
        assertions: Vec::new(),
        extractors: Vec::new(),
        scripts: Default::default(),
        settings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ParamKind;

    #[test]
    fn parses_realistic_multiline_command() {
        let cmd = r#"curl -X POST 'https://api.example.com/v1/users?active=true' \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json' \
  -u 'alice:s3cret' \
  -b 'session=abc123' \
  -A 'ForgeTest/1.0' \
  -e 'https://example.com/ref' \
  -L \
  -k \
  --compressed \
  -d '{"name":"Ada"}'"#;
        let def = parse_curl(cmd).expect("should parse");
        assert_eq!(def.method, Method::Post);
        assert_eq!(def.url, "https://api.example.com/v1/users?active=true");
        assert_eq!(def.auth, AuthConfig::Basic { username: "alice".into(), password: "s3cret".into() });
        assert_eq!(def.settings.follow_redirects, Some(true));
        assert_eq!(def.settings.verify_tls, Some(false));
        assert!(matches!(&def.body, BodyDef::Json { text } if text == r#"{"name":"Ada"}"#));
        let header = |name: &str| def.headers.iter().find(|h| h.key.eq_ignore_ascii_case(name)).map(|h| h.value.clone());
        assert_eq!(header("Cookie"), Some("session=abc123".to_string()));
        assert_eq!(header("User-Agent"), Some("ForgeTest/1.0".to_string()));
        assert_eq!(header("Referer"), Some("https://example.com/ref".to_string()));
        assert_eq!(header("Accept-Encoding"), Some("gzip, deflate, br".to_string()));
        assert_eq!(def.name, "POST api.example.com/v1/users");
    }

    #[test]
    fn data_urlencode_encodes_value() {
        let def = parse_curl("curl https://example.com --data-urlencode 'q=hello world&more'").unwrap();
        match &def.body {
            BodyDef::Raw { text, .. } => assert_eq!(text, "q=hello%20world%26more"),
            other => panic!("expected Raw body, got {other:?}"),
        }
        assert_eq!(def.method, Method::Post);
    }

    #[test]
    fn data_urlencode_bare_content() {
        let def = parse_curl("curl https://example.com --data-urlencode 'a b'").unwrap();
        match &def.body {
            BodyDef::Raw { text, .. } => assert_eq!(text, "a%20b"),
            other => panic!("unexpected body {other:?}"),
        }
    }

    #[test]
    fn get_flag_moves_data_to_query() {
        let def = parse_curl("curl -G https://example.com/search -d 'q=rust' -d 'lang=en'").unwrap();
        assert_eq!(def.method, Method::Get);
        assert_eq!(def.url, "https://example.com/search?q=rust&lang=en");
        assert_eq!(def.body, BodyDef::None);
    }

    #[test]
    fn multipart_file_and_type() {
        let def = parse_curl(
            "curl https://example.com/upload -F 'file=@/tmp/a.png;type=image/png' -F 'label=hello'",
        )
        .unwrap();
        match &def.body {
            BodyDef::Multipart { parts } => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0].name, "file");
                assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));
                assert!(matches!(&parts[0].content, PartContent::File { path } if path == "/tmp/a.png"));
                assert_eq!(parts[1].name, "label");
                assert!(matches!(&parts[1].content, PartContent::Text { value } if value == "hello"));
            }
            other => panic!("expected multipart, got {other:?}"),
        }
        assert_eq!(def.method, Method::Post);
    }

    #[test]
    fn quoted_values_with_spaces_and_single_quotes() {
        let def = parse_curl(r#"curl "https://example.com/search" -H "X-Note: it's fine, spaced value""#).unwrap();
        let note = def.headers.iter().find(|h| h.key == "X-Note").unwrap();
        assert_eq!(note.value, "it's fine, spaced value");
    }

    #[test]
    fn missing_scheme_gets_https_prefix() {
        let def = parse_curl("curl example.com/api/x").unwrap();
        assert_eq!(def.url, "https://example.com/api/x");
    }

    #[test]
    fn attached_short_flags_and_clusters() {
        let def = parse_curl("curl -sSL -XPOST https://example.com -d'{}'").unwrap();
        assert_eq!(def.method, Method::Post);
        assert_eq!(def.settings.follow_redirects, Some(true));
        assert!(matches!(
            &def.body,
            BodyDef::Raw { text, language: RawLanguage::Json } if text == "{}"
        ));
    }

    #[test]
    fn unknown_boolean_flag_is_skipped() {
        let def = parse_curl("curl --anything-weird https://example.com").unwrap();
        assert_eq!(def.url, "https://example.com");
        assert_eq!(def.method, Method::Get);
    }

    #[test]
    fn unknown_value_flag_does_not_swallow_url() {
        let def = parse_curl("curl --max-time 30 https://example.com/x").unwrap();
        assert_eq!(def.url, "https://example.com/x");
    }

    #[test]
    fn lone_header_semicolon_is_empty_value() {
        let def = parse_curl(r#"curl https://example.com -H "X-Empty;""#).unwrap();
        let h = def.headers.iter().find(|h| h.key == "X-Empty").unwrap();
        assert_eq!(h.value, "");
    }

    #[test]
    fn head_flag_sets_method() {
        let def = parse_curl("curl -I https://example.com").unwrap();
        assert_eq!(def.method, Method::Head);
    }

    #[test]
    fn params_stay_in_url_not_exploded() {
        let def = parse_curl("curl 'https://example.com/x?a=1&b=2'").unwrap();
        assert!(def.params.is_empty());
        assert_eq!(def.url, "https://example.com/x?a=1&b=2");
        let _ = ParamKind::Query; // referenced to keep import obviously intentional
    }

    #[test]
    fn missing_url_errors() {
        let err = parse_curl("curl -X GET").unwrap_err();
        assert_eq!(err, CurlParseError::MissingUrl);
    }

    #[test]
    fn unknown_value_flag_with_dashdash_prefix_does_not_swallow_url() {
        // --proxy-cacert wasn't in VALUE_LONG_FLAGS before; its value token
        // (a path) used to be mistaken for the positional URL.
        let def = parse_curl("curl --proxy-cacert /etc/ssl/cert.pem https://api.example.com/data").unwrap();
        assert_eq!(def.url, "https://api.example.com/data");
    }

    #[test]
    fn data_at_file_reference_emits_note() {
        let def = parse_curl("curl https://example.com -d @payload.json").unwrap();
        assert!(
            def.description.contains("body references file @payload.json; file contents were not imported"),
            "unexpected description: {}",
            def.description
        );
        match &def.body {
            BodyDef::Raw { text, .. } => assert_eq!(text, "@payload.json"),
            other => panic!("expected Raw body, got {other:?}"),
        }
    }

    #[test]
    fn data_urlencode_named_at_file_reference_emits_note() {
        let def = parse_curl("curl https://example.com --data-urlencode field=@payload.json").unwrap();
        assert!(
            def.description.contains("body references file @payload.json; file contents were not imported"),
            "unexpected description: {}",
            def.description
        );
    }

    #[test]
    fn data_raw_at_prefix_is_not_treated_as_file_reference() {
        let def = parse_curl("curl https://example.com --data-raw @not-a-file").unwrap();
        assert!(def.description.is_empty());
    }
}
