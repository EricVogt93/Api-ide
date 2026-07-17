//! Generate runnable code snippets from a [`RequestDef`].

use crate::convert::common::{append_query, enabled_headers, percent_encode_form, query_pairs, shell_quote};
use crate::model::{ApiKeyPlacement, AuthConfig, BodyDef, Method, MultipartPart, PartContent, RawLanguage, RequestDef};

/// Target language/library for [`generate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnippetLang {
    /// Browser/Node `fetch`, `async`/`await` style.
    JsFetch,
    /// Node `axios`.
    Axios,
    /// Python `requests`.
    PythonRequests,
    /// The `httpie` CLI (`http ...`).
    Httpie,
    /// Go `net/http`.
    Go,
    /// Java 11+ `java.net.http.HttpClient`.
    JavaHttpClient,
}

impl SnippetLang {
    /// All supported languages, in a stable display order.
    pub fn all() -> [SnippetLang; 6] {
        [
            SnippetLang::JsFetch,
            SnippetLang::Axios,
            SnippetLang::PythonRequests,
            SnippetLang::Httpie,
            SnippetLang::Go,
            SnippetLang::JavaHttpClient,
        ]
    }

    /// Human-readable label for UI pickers.
    pub fn label(&self) -> &'static str {
        match self {
            SnippetLang::JsFetch => "JavaScript (fetch)",
            SnippetLang::Axios => "JavaScript (axios)",
            SnippetLang::PythonRequests => "Python (requests)",
            SnippetLang::Httpie => "HTTPie",
            SnippetLang::Go => "Go (net/http)",
            SnippetLang::JavaHttpClient => "Java (HttpClient)",
        }
    }
}

/// A request boiled down to the shape every snippet generator needs.
struct View {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    basic_auth: Option<(String, String)>,
    body: Body,
}

enum Body {
    None,
    /// A parsed JSON value, ready to embed as a literal.
    Json(serde_json::Value),
    Text(String),
    Form(Vec<(String, String)>),
    Multipart(Vec<MultipartPart>),
}

fn build_view(def: &RequestDef) -> View {
    let mut headers = enabled_headers(def);
    let mut query = query_pairs(def);
    let mut basic_auth = None;

    match &def.auth {
        AuthConfig::Basic { username, password } => {
            basic_auth = Some((username.clone(), password.clone()));
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
        // Challenge-response (Digest), token exchange (OAuth2) and request
        // signing (SigV4) can't be expressed as static snippet headers.
        AuthConfig::OAuth2ClientCredentials { .. }
        | AuthConfig::OAuth2AuthCode { .. }
        | AuthConfig::Digest { .. }
        | AuthConfig::AwsSigV4 { .. } => {}
    }

    let url = append_query(&def.url, &query);

    let body = match &def.body {
        BodyDef::None => Body::None,
        BodyDef::Json { text } => match serde_json::from_str(text) {
            Ok(v) => Body::Json(v),
            Err(_) => Body::Text(text.clone()),
        },
        BodyDef::Raw { text, language } => {
            if *language == RawLanguage::Json {
                match serde_json::from_str(text) {
                    Ok(v) => Body::Json(v),
                    Err(_) => Body::Text(text.clone()),
                }
            } else {
                Body::Text(text.clone())
            }
        }
        BodyDef::Xml { text } => Body::Text(text.clone()),
        BodyDef::FormUrlencoded { fields } => Body::Form(
            fields.iter().filter(|f| f.is_active()).map(|f| (f.key.clone(), f.value.clone())).collect(),
        ),
        BodyDef::Multipart { parts } => {
            Body::Multipart(parts.iter().filter(|p| p.enabled).cloned().collect())
        }
        BodyDef::GraphQl { query: gql_query, variables, operation_name } => {
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
            }
            let json = crate::convert::common::graphql_json_body(gql_query, variables, operation_name);
            Body::Json(serde_json::from_str(&json).unwrap_or(serde_json::Value::Null))
        }
        BodyDef::Binary { path } => Body::Text(format!("@{path}")),
    };

    View { method: def.method, url, headers, basic_auth, body }
}

/// A JSON/JS/Python/Java/Go compatible double-quoted string literal.
fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s.replace('"', "\\\"")))
}

fn python_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => quote(s),
        serde_json::Value::Array(items) => {
            format!("[{}]", items.iter().map(python_literal).collect::<Vec<_>>().join(", "))
        }
        serde_json::Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, val)| format!("{}: {}", quote(k), python_literal(val)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn go_raw_string(s: &str) -> String {
    if s.contains('`') {
        quote(s)
    } else {
        format!("`{s}`")
    }
}

/// Generate a runnable code snippet for `def` in the given language.
pub fn generate(def: &RequestDef, lang: SnippetLang) -> String {
    let view = build_view(def);
    match lang {
        SnippetLang::JsFetch => js_fetch(&view),
        SnippetLang::Axios => axios(&view),
        SnippetLang::PythonRequests => python_requests(&view),
        SnippetLang::Httpie => httpie(&view),
        SnippetLang::Go => go_net_http(&view),
        SnippetLang::JavaHttpClient => java_http_client(&view),
    }
}

fn js_fetch(v: &View) -> String {
    let mut header_entries: Vec<String> = v
        .headers
        .iter()
        .map(|(k, val)| format!("      {}: {}", quote(k), quote(val)))
        .collect();
    if let Some((u, p)) = &v.basic_auth {
        header_entries
            .push(format!("      Authorization: 'Basic ' + btoa({})", quote(&format!("{u}:{p}"))));
    }
    let headers_block = if header_entries.is_empty() {
        "    headers: {},\n".to_string()
    } else {
        format!("    headers: {{\n{}\n    }},\n", header_entries.join(",\n"))
    };

    let body_line = match &v.body {
        Body::None => String::new(),
        Body::Json(val) => format!("    body: JSON.stringify({val}),\n"),
        Body::Text(text) => format!("    body: {},\n", quote(text)),
        Body::Form(fields) => {
            let entries = fields
                .iter()
                .map(|(k, val)| format!("{}: {}", quote(k), quote(val)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("    body: new URLSearchParams({{ {entries} }}).toString(),\n")
        }
        Body::Multipart(parts) => {
            let mut s = String::from("    body: (() => {\n      const form = new FormData();\n");
            for p in parts {
                match &p.content {
                    PartContent::Text { value } => {
                        s.push_str(&format!("      form.append({}, {});\n", quote(&p.name), quote(value)));
                    }
                    PartContent::File { path } => {
                        s.push_str(&format!(
                            "      form.append({}, fs.createReadStream({}));\n",
                            quote(&p.name),
                            quote(path)
                        ));
                    }
                }
            }
            s.push_str("      return form;\n    })(),\n");
            s
        }
    };

    format!(
        "async function run() {{\n  const response = await fetch({}, {{\n    method: {},\n{}{}  }});\n  const data = await response.text();\n  console.log(response.status, data);\n}}\n\nrun();",
        quote(&v.url),
        quote(v.method.as_str()),
        headers_block,
        body_line
    )
}

fn axios(v: &View) -> String {
    let mut lines = vec!["const axios = require('axios');".to_string(), String::new(), "axios.request({".to_string()];
    lines.push(format!("  method: {},", quote(&v.method.as_str().to_ascii_lowercase())));
    lines.push(format!("  url: {},", quote(&v.url)));
    if !v.headers.is_empty() {
        lines.push("  headers: {".to_string());
        let last = v.headers.len() - 1;
        for (i, (k, val)) in v.headers.iter().enumerate() {
            let comma = if i == last { "" } else { "," };
            lines.push(format!("    {}: {}{}", quote(k), quote(val), comma));
        }
        lines.push("  },".to_string());
    }
    if let Some((u, p)) = &v.basic_auth {
        lines.push("  auth: {".to_string());
        lines.push(format!("    username: {},", quote(u)));
        lines.push(format!("    password: {}", quote(p)));
        lines.push("  },".to_string());
    }
    match &v.body {
        Body::None => {}
        Body::Json(val) => lines.push(format!("  data: {val},")),
        Body::Text(text) => lines.push(format!("  data: {},", quote(text))),
        Body::Form(fields) => {
            lines.push("  data: new URLSearchParams({".to_string());
            let last = fields.len().saturating_sub(1);
            for (i, (k, val)) in fields.iter().enumerate() {
                let comma = if i == last { "" } else { "," };
                lines.push(format!("    {}: {}{}", quote(k), quote(val), comma));
            }
            lines.push("  }).toString(),".to_string());
        }
        Body::Multipart(parts) => {
            lines.push("  data: (() => {".to_string());
            lines.push("    const form = new FormData();".to_string());
            for p in parts {
                match &p.content {
                    PartContent::Text { value } => {
                        lines.push(format!("    form.append({}, {});", quote(&p.name), quote(value)))
                    }
                    PartContent::File { path } => lines.push(format!(
                        "    form.append({}, fs.createReadStream({}));",
                        quote(&p.name),
                        quote(path)
                    )),
                }
            }
            lines.push("    return form;".to_string());
            lines.push("  })(),".to_string());
        }
    }
    lines.push("})".to_string());
    lines.push("  .then((response) => console.log(response.data))".to_string());
    lines.push("  .catch((error) => console.error(error));".to_string());
    lines.join("\n")
}

fn python_requests(v: &View) -> String {
    let mut lines = vec!["import requests".to_string(), String::new()];
    let method_lower = v.method.as_str().to_ascii_lowercase();
    lines.push(format!("response = requests.{method_lower}("));
    lines.push(format!("    {},", quote(&v.url)));
    if !v.headers.is_empty() {
        lines.push("    headers={".to_string());
        let last = v.headers.len() - 1;
        for (i, (k, val)) in v.headers.iter().enumerate() {
            let comma = if i == last { "" } else { "," };
            lines.push(format!("        {}: {}{}", quote(k), quote(val), comma));
        }
        lines.push("    },".to_string());
    }
    if let Some((u, p)) = &v.basic_auth {
        lines.push(format!("    auth=({}, {}),", quote(u), quote(p)));
    }
    match &v.body {
        Body::None => {}
        Body::Json(val) => lines.push(format!("    json={},", python_literal(val))),
        Body::Text(text) => lines.push(format!("    data={},", quote(text))),
        Body::Form(fields) => {
            lines.push("    data={".to_string());
            let last = fields.len().saturating_sub(1);
            for (i, (k, val)) in fields.iter().enumerate() {
                let comma = if i == last { "" } else { "," };
                lines.push(format!("        {}: {}{}", quote(k), quote(val), comma));
            }
            lines.push("    },".to_string());
        }
        Body::Multipart(parts) => {
            lines.push("    files={".to_string());
            for p in parts {
                match &p.content {
                    PartContent::Text { value } => {
                        lines.push(format!("        {}: (None, {}),", quote(&p.name), quote(value)))
                    }
                    PartContent::File { path } => {
                        lines.push(format!("        {}: open({}, 'rb'),", quote(&p.name), quote(path)))
                    }
                }
            }
            lines.push("    },".to_string());
        }
    }
    lines.push(")".to_string());
    lines.push(String::new());
    lines.push("print(response.status_code)".to_string());
    lines.push("print(response.text)".to_string());
    lines.join("\n")
}

fn httpie(v: &View) -> String {
    let mut args: Vec<String> = vec!["http".to_string()];
    if let Some((u, p)) = &v.basic_auth {
        args.push("-a".to_string());
        args.push(shell_quote(&format!("{u}:{p}")));
    }
    args.push(v.method.as_str().to_string());
    args.push(shell_quote(&v.url));
    for (k, val) in &v.headers {
        args.push(shell_quote(&format!("{k}:{val}")));
    }
    match &v.body {
        Body::None => {}
        Body::Json(serde_json::Value::Object(map)) => {
            for (k, val) in map {
                args.push(shell_quote(&format!("{k}:={val}")));
            }
        }
        Body::Json(other) => {
            args.push("--raw".to_string());
            args.push(shell_quote(&other.to_string()));
        }
        Body::Text(text) => {
            args.push("--raw".to_string());
            args.push(shell_quote(text));
        }
        Body::Form(fields) => {
            for (k, val) in fields {
                args.push(shell_quote(&format!("{k}={val}")));
            }
        }
        Body::Multipart(parts) => {
            args.push("--multipart".to_string());
            for p in parts {
                match &p.content {
                    PartContent::Text { value } => args.push(shell_quote(&format!("{}={}", p.name, value))),
                    PartContent::File { path } => args.push(shell_quote(&format!("{}@{}", p.name, path))),
                }
            }
        }
    }
    args.join(" ")
}

fn go_net_http(v: &View) -> String {
    let (body_decl, body_arg, mut extra_imports): (String, String, Vec<&'static str>) = match &v.body {
        Body::None => (String::new(), "nil".to_string(), vec![]),
        Body::Json(val) => (
            format!("\tbody := strings.NewReader({})\n", go_raw_string(&val.to_string())),
            "body".to_string(),
            vec!["strings"],
        ),
        Body::Text(text) => (
            format!("\tbody := strings.NewReader({})\n", go_raw_string(text)),
            "body".to_string(),
            vec!["strings"],
        ),
        Body::Form(fields) => {
            let mut decl = String::from("\tform := url.Values{}\n");
            for (k, val) in fields {
                decl.push_str(&format!("\tform.Set({}, {})\n", quote(k), quote(val)));
            }
            decl.push_str("\tbody := strings.NewReader(form.Encode())\n");
            (decl, "body".to_string(), vec!["strings", "net/url"])
        }
        Body::Multipart(parts) => {
            let mut decl =
                String::from("\tvar buf bytes.Buffer\n\twriter := multipart.NewWriter(&buf)\n");
            let mut needs_os = false;
            for p in parts {
                match &p.content {
                    PartContent::Text { value } => {
                        decl.push_str(&format!(
                            "\twriter.WriteField({}, {})\n",
                            quote(&p.name),
                            quote(value)
                        ));
                    }
                    PartContent::File { path } => {
                        needs_os = true;
                        decl.push_str(&format!(
                            "\tfw, _ := writer.CreateFormFile({}, {})\n\tf, _ := os.Open({})\n\tio.Copy(fw, f)\n\tf.Close()\n",
                            quote(&p.name),
                            quote(path),
                            quote(path)
                        ));
                    }
                }
            }
            decl.push_str("\twriter.Close()\n");
            let mut imports = vec!["bytes", "mime/multipart"];
            if needs_os {
                imports.push("os");
            }
            (decl, "&buf".to_string(), imports)
        }
    };

    extra_imports.push("fmt");
    extra_imports.push("io");
    extra_imports.push("net/http");
    extra_imports.sort_unstable();
    extra_imports.dedup();
    let import_block =
        extra_imports.iter().map(|i| format!("\t\"{i}\"")).collect::<Vec<_>>().join("\n");

    let mut header_lines = String::new();
    for (k, val) in &v.headers {
        header_lines.push_str(&format!("\treq.Header.Set({}, {})\n", quote(k), quote(val)));
    }
    let auth_line = match &v.basic_auth {
        Some((u, p)) => format!("\treq.SetBasicAuth({}, {})\n", quote(u), quote(p)),
        None => String::new(),
    };

    format!(
        "package main\n\nimport (\n{import_block}\n)\n\nfunc main() {{\n{body_decl}\treq, err := http.NewRequest({}, {}, {body_arg})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n{header_lines}{auth_line}\n\tclient := &http.Client{{}}\n\tresp, err := client.Do(req)\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer resp.Body.Close()\n\n\trespBody, _ := io.ReadAll(resp.Body)\n\tfmt.Println(resp.StatusCode, string(respBody))\n}}",
        quote(v.method.as_str()),
        quote(&v.url)
    )
}

fn java_http_client(v: &View) -> String {
    let mut lines = vec![
        "import java.net.URI;".to_string(),
        "import java.net.http.HttpClient;".to_string(),
        "import java.net.http.HttpRequest;".to_string(),
        "import java.net.http.HttpResponse;".to_string(),
    ];
    if v.basic_auth.is_some() {
        lines.push("import java.util.Base64;".to_string());
    }
    lines.push(String::new());
    lines.push("public class Main {".to_string());
    lines.push("    public static void main(String[] args) throws Exception {".to_string());
    lines.push("        HttpClient client = HttpClient.newHttpClient();".to_string());
    if let Some((u, p)) = &v.basic_auth {
        lines.push(format!(
            "        String credentials = Base64.getEncoder().encodeToString({}.getBytes());",
            quote(&format!("{u}:{p}"))
        ));
    }

    let (body_publisher, body_decl): (String, Option<String>) = match &v.body {
        Body::None => ("HttpRequest.BodyPublishers.noBody()".to_string(), None),
        Body::Json(val) => (
            "HttpRequest.BodyPublishers.ofString(payload)".to_string(),
            Some(format!("        String payload = {};", quote(&val.to_string()))),
        ),
        Body::Text(text) => (
            "HttpRequest.BodyPublishers.ofString(payload)".to_string(),
            Some(format!("        String payload = {};", quote(text))),
        ),
        Body::Form(fields) => {
            let encoded = fields
                .iter()
                .map(|(k, val)| format!("{}={}", percent_encode_form(k), percent_encode_form(val)))
                .collect::<Vec<_>>()
                .join("&");
            (
                "HttpRequest.BodyPublishers.ofString(payload)".to_string(),
                Some(format!("        String payload = {};", quote(&encoded))),
            )
        }
        Body::Multipart(_) => (
            "HttpRequest.BodyPublishers.ofString(payload) /* TODO: build a real multipart body */"
                .to_string(),
            Some("        String payload = \"\";".to_string()),
        ),
    };
    if let Some(decl) = &body_decl {
        lines.push(decl.clone());
    }

    lines.push("        HttpRequest request = HttpRequest.newBuilder()".to_string());
    lines.push(format!("                .uri(URI.create({}))", quote(&v.url)));
    for (k, val) in &v.headers {
        lines.push(format!("                .header({}, {})", quote(k), quote(val)));
    }
    if v.basic_auth.is_some() {
        lines.push("                .header(\"Authorization\", \"Basic \" + credentials)".to_string());
    }
    lines.push(format!("                .method({}, {})", quote(v.method.as_str()), body_publisher));
    lines.push("                .build();".to_string());
    lines.push(String::new());
    lines.push(
        "        HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());"
            .to_string(),
    );
    lines.push("        System.out.println(response.statusCode());".to_string());
    lines.push("        System.out.println(response.body());".to_string());
    lines.push("    }".to_string());
    lines.push("}".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KeyValue, Param, ParamKind, RequestDef};

    fn sample() -> RequestDef {
        let mut def = RequestDef::new("Create user", Method::Post, "https://api.example.com/users");
        def.headers.push(KeyValue::new("Content-Type", "application/json"));
        def.auth = AuthConfig::Basic { username: "alice".into(), password: "s3cret".into() };
        def.body = BodyDef::Json { text: r#"{"name":"Ada"}"#.to_string() };
        def.params.push(Param { kv: KeyValue::new("verbose", "1"), kind: ParamKind::Query });
        def
    }

    #[test]
    fn all_returns_six_languages() {
        assert_eq!(SnippetLang::all().len(), 6);
        for lang in SnippetLang::all() {
            assert!(!lang.label().is_empty());
        }
    }

    #[test]
    fn disabled_rows_are_skipped_everywhere() {
        let mut def = sample();
        def.headers.push(KeyValue { key: "X-Off".into(), value: "nope".into(), description: String::new(), enabled: false });
        def.params.push(Param { kv: KeyValue { key: "off".into(), value: "1".into(), description: String::new(), enabled: false }, kind: ParamKind::Query });
        for lang in SnippetLang::all() {
            let out = generate(&def, lang);
            assert!(!out.contains("X-Off"), "{lang:?} leaked disabled header");
            assert!(!out.contains("off=1"), "{lang:?} leaked disabled param");
        }
    }

    #[test]
    fn query_params_land_in_url_for_every_lang() {
        let def = sample();
        for lang in SnippetLang::all() {
            let out = generate(&def, lang);
            assert!(out.contains("verbose"), "{lang:?} missing query param");
        }
    }

    #[test]
    fn golden_js_fetch() {
        let out = generate(&sample(), SnippetLang::JsFetch);
        let expected = "async function run() {\n  const response = await fetch(\"https://api.example.com/users?verbose=1\", {\n    method: \"POST\",\n    headers: {\n      \"Content-Type\": \"application/json\",\n      Authorization: 'Basic ' + btoa(\"alice:s3cret\")\n    },\n    body: JSON.stringify({\"name\":\"Ada\"}),\n  });\n  const data = await response.text();\n  console.log(response.status, data);\n}\n\nrun();";
        assert_eq!(out, expected);
    }

    #[test]
    fn golden_axios() {
        let out = generate(&sample(), SnippetLang::Axios);
        let expected = "const axios = require('axios');\n\naxios.request({\n  method: \"post\",\n  url: \"https://api.example.com/users?verbose=1\",\n  headers: {\n    \"Content-Type\": \"application/json\"\n  },\n  auth: {\n    username: \"alice\",\n    password: \"s3cret\"\n  },\n  data: {\"name\":\"Ada\"},\n})\n  .then((response) => console.log(response.data))\n  .catch((error) => console.error(error));";
        assert_eq!(out, expected);
    }

    #[test]
    fn golden_python_requests() {
        let out = generate(&sample(), SnippetLang::PythonRequests);
        let expected = "import requests\n\nresponse = requests.post(\n    \"https://api.example.com/users?verbose=1\",\n    headers={\n        \"Content-Type\": \"application/json\"\n    },\n    auth=(\"alice\", \"s3cret\"),\n    json={\"name\": \"Ada\"},\n)\n\nprint(response.status_code)\nprint(response.text)";
        assert_eq!(out, expected);
    }

    #[test]
    fn golden_httpie() {
        let out = generate(&sample(), SnippetLang::Httpie);
        let expected = "http -a 'alice:s3cret' POST 'https://api.example.com/users?verbose=1' 'Content-Type:application/json' 'name:=\"Ada\"'";
        assert_eq!(out, expected);
    }

    #[test]
    fn golden_go() {
        let out = generate(&sample(), SnippetLang::Go);
        let expected = "package main\n\nimport (\n\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n\t\"strings\"\n)\n\nfunc main() {\n\tbody := strings.NewReader(`{\"name\":\"Ada\"}`)\n\treq, err := http.NewRequest(\"POST\", \"https://api.example.com/users?verbose=1\", body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\treq.Header.Set(\"Content-Type\", \"application/json\")\n\treq.SetBasicAuth(\"alice\", \"s3cret\")\n\n\tclient := &http.Client{}\n\tresp, err := client.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer resp.Body.Close()\n\n\trespBody, _ := io.ReadAll(resp.Body)\n\tfmt.Println(resp.StatusCode, string(respBody))\n}";
        assert_eq!(out, expected);
    }

    #[test]
    fn golden_java() {
        let out = generate(&sample(), SnippetLang::JavaHttpClient);
        let expected = "import java.net.URI;\nimport java.net.http.HttpClient;\nimport java.net.http.HttpRequest;\nimport java.net.http.HttpResponse;\nimport java.util.Base64;\n\npublic class Main {\n    public static void main(String[] args) throws Exception {\n        HttpClient client = HttpClient.newHttpClient();\n        String credentials = Base64.getEncoder().encodeToString(\"alice:s3cret\".getBytes());\n        String payload = \"{\\\"name\\\":\\\"Ada\\\"}\";\n        HttpRequest request = HttpRequest.newBuilder()\n                .uri(URI.create(\"https://api.example.com/users?verbose=1\"))\n                .header(\"Content-Type\", \"application/json\")\n                .header(\"Authorization\", \"Basic \" + credentials)\n                .method(\"POST\", HttpRequest.BodyPublishers.ofString(payload))\n                .build();\n\n        HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());\n        System.out.println(response.statusCode());\n        System.out.println(response.body());\n    }\n}";
        assert_eq!(out, expected);
    }

    #[test]
    fn form_body_uses_url_search_params() {
        let mut def = sample();
        def.body = BodyDef::FormUrlencoded { fields: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2")] };
        let js = generate(&def, SnippetLang::JsFetch);
        assert!(js.contains("URLSearchParams"));
        let py = generate(&def, SnippetLang::PythonRequests);
        assert!(py.contains("data={"));
    }

    #[test]
    fn multipart_body_generates_form_data() {
        let mut def = sample();
        def.body = BodyDef::Multipart {
            parts: vec![crate::model::MultipartPart {
                name: "file".to_string(),
                content: PartContent::File { path: "/tmp/a.png".to_string() },
                content_type: None,
                enabled: true,
            }],
        };
        let go = generate(&def, SnippetLang::Go);
        assert!(go.contains("multipart.NewWriter"));
        let httpie_out = generate(&def, SnippetLang::Httpie);
        assert!(httpie_out.contains("file@/tmp/a.png"));
    }
}
