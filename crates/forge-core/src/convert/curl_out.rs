//! Render a [`RequestDef`] as a curl command line.

use crate::convert::common::{append_query, enabled_headers, graphql_json_body, has_header, query_pairs, shell_quote};
use crate::model::{ApiKeyPlacement, AuthConfig, BodyDef, PartContent, RequestDef};

/// Options controlling how [`to_curl`] renders a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurlExportOptions {
    /// Split the command across multiple lines with `\` continuations.
    pub multiline: bool,
    /// Prefer long flag names (`--header`, `--request`) over short ones.
    pub long_flags: bool,
}

impl Default for CurlExportOptions {
    fn default() -> Self {
        Self { multiline: true, long_flags: false }
    }
}

/// Render a [`RequestDef`] as a curl command line.
///
/// Enabled query params are appended to the URL, enabled headers are
/// emitted with `-H`, auth is translated to `-u`/`Authorization`/query
/// param as appropriate, and the body is emitted with the flag that best
/// matches its shape. `{{variables}}` in the URL, headers and body are
/// kept verbatim.
pub fn to_curl(def: &RequestDef, opts: &CurlExportOptions) -> String {
    let method_flag = if opts.long_flags { "--request" } else { "-X" };
    let header_flag = if opts.long_flags { "--header" } else { "-H" };

    let mut headers = enabled_headers(def);
    let mut query = query_pairs(def);
    let mut auth_chunk: Option<String> = None;

    match &def.auth {
        AuthConfig::Basic { username, password } => {
            let flag = if opts.long_flags { "--user" } else { "-u" };
            auth_chunk =
                Some(format!("{flag} {}", shell_quote(&format!("{username}:{password}"))));
        }
        AuthConfig::Bearer { token, prefix } => {
            let prefix = prefix.clone().unwrap_or_else(|| "Bearer".to_string());
            headers.push(("Authorization".to_string(), format!("{prefix} {token}")));
        }
        AuthConfig::ApiKey { key, value, placement } => match placement {
            ApiKeyPlacement::Header => headers.push((key.clone(), value.clone())),
            ApiKeyPlacement::Query => query.push((key.clone(), value.clone())),
        },
        AuthConfig::None | AuthConfig::Inherit => {}
        // OAuth2 flows need a live token exchange; not representable as a
        // static curl flag, so they're left for the caller to pre-resolve.
        AuthConfig::OAuth2ClientCredentials { .. } | AuthConfig::OAuth2AuthCode { .. } => {}
    }

    let url = append_query(&def.url, &query);

    let mut body_chunks: Vec<String> = Vec::new();
    match &def.body {
        BodyDef::None => {}
        BodyDef::Raw { text, .. } | BodyDef::Json { text } | BodyDef::Xml { text } => {
            body_chunks.push(format!("--data-raw {}", shell_quote(text)));
        }
        BodyDef::FormUrlencoded { fields } => {
            for f in fields.iter().filter(|f| f.is_active()) {
                body_chunks.push(format!(
                    "--data-urlencode {}",
                    shell_quote(&format!("{}={}", f.key, f.value))
                ));
            }
        }
        BodyDef::Multipart { parts } => {
            let flag = if opts.long_flags { "--form" } else { "-F" };
            for p in parts.iter().filter(|p| p.enabled) {
                let mut value = match &p.content {
                    PartContent::Text { value } => format!("{}={}", p.name, value),
                    PartContent::File { path } => format!("{}=@{}", p.name, path),
                };
                if let Some(ct) = &p.content_type {
                    value.push_str(&format!(";type={ct}"));
                }
                body_chunks.push(format!("{flag} {}", shell_quote(&value)));
            }
        }
        BodyDef::GraphQl { query: gql_query, variables, operation_name } => {
            if !has_header(&headers, "content-type") {
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
            }
            let json = graphql_json_body(gql_query, variables, operation_name);
            body_chunks.push(format!("--data-raw {}", shell_quote(&json)));
        }
        BodyDef::Binary { path } => {
            body_chunks.push(format!("--data-binary {}", shell_quote(&format!("@{path}"))));
        }
    }

    let mut chunks: Vec<String> = Vec::new();
    chunks.push("curl".to_string());
    chunks.push(format!("{method_flag} {}", def.method.as_str()));
    chunks.push(shell_quote(&url));
    for (k, v) in &headers {
        chunks.push(format!("{header_flag} {}", shell_quote(&format!("{k}: {v}"))));
    }
    if let Some(auth_chunk) = auth_chunk {
        chunks.push(auth_chunk);
    }
    chunks.extend(body_chunks);
    if def.settings.follow_redirects == Some(true) {
        chunks.push(if opts.long_flags { "--location".to_string() } else { "-L".to_string() });
    }
    if def.settings.verify_tls == Some(false) {
        chunks.push(if opts.long_flags { "--insecure".to_string() } else { "-k".to_string() });
    }

    let sep = if opts.multiline { " \\\n  " } else { " " };
    chunks.join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::curl_in::parse_curl;
    use crate::model::{KeyValue, Method, Param, ParamKind, RequestDef};

    fn sample() -> RequestDef {
        let mut def = RequestDef::new("Create user", Method::Post, "https://api.example.com/users");
        def.headers.push(KeyValue::new("Content-Type", "application/json"));
        def.auth = AuthConfig::Basic { username: "alice".into(), password: "s3cret".into() };
        def.body = BodyDef::Json { text: r#"{"name":"Ada"}"#.to_string() };
        def.params.push(Param { kv: KeyValue::new("verbose", "1"), kind: ParamKind::Query });
        def
    }

    #[test]
    fn emits_expected_flags() {
        let out = to_curl(&sample(), &CurlExportOptions { multiline: false, long_flags: false });
        assert!(out.starts_with("curl -X POST "));
        assert!(out.contains("'https://api.example.com/users?verbose=1'"));
        assert!(out.contains("-H 'Content-Type: application/json'"));
        assert!(out.contains("-u 'alice:s3cret'"));
        assert!(out.contains(r#"--data-raw '{"name":"Ada"}'"#));
    }

    #[test]
    fn multiline_uses_continuations() {
        let out = to_curl(&sample(), &CurlExportOptions::default());
        assert!(out.contains(" \\\n  "));
    }

    #[test]
    fn long_flags_option() {
        let out = to_curl(&sample(), &CurlExportOptions { multiline: false, long_flags: true });
        assert!(out.contains("--request POST"));
        assert!(out.contains("--header 'Content-Type: application/json'"));
        assert!(out.contains("--user 'alice:s3cret'"));
    }

    #[test]
    fn escapes_single_quotes_in_body() {
        let mut def = sample();
        def.body = BodyDef::Raw { text: "it's a test".into(), language: Default::default() };
        let out = to_curl(&def, &CurlExportOptions { multiline: false, long_flags: false });
        assert!(out.contains(r#"'it'"'"'s a test'"#));
    }

    #[test]
    fn follow_redirects_and_insecure_flags() {
        let mut def = sample();
        def.settings.follow_redirects = Some(true);
        def.settings.verify_tls = Some(false);
        let out = to_curl(&def, &CurlExportOptions { multiline: false, long_flags: false });
        assert!(out.contains(" -L"));
        assert!(out.contains(" -k"));
    }

    #[test]
    fn roundtrip_preserves_method_url_headers_body() {
        let def = sample();
        let curl = to_curl(&def, &CurlExportOptions { multiline: true, long_flags: false });
        let reparsed = parse_curl(&curl).expect("roundtrip should parse");
        assert_eq!(reparsed.method, def.method);
        assert_eq!(reparsed.url, "https://api.example.com/users?verbose=1");
        assert_eq!(
            reparsed.headers.iter().find(|h| h.key == "Content-Type").map(|h| h.value.clone()),
            Some("application/json".to_string())
        );
        match (&reparsed.body, &def.body) {
            (BodyDef::Json { text: a }, BodyDef::Json { text: b }) => assert_eq!(a, b),
            other => panic!("body mismatch: {other:?}"),
        }
        assert_eq!(reparsed.auth, def.auth);
    }
}
