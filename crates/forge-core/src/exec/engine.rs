//! The HTTP execution engine: turns a [`ResolvedRequest`] into an
//! [`ExecutionResult`], handling cookies, manual redirects, timing,
//! timeouts and cancellation.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::prelude::{Engine as _, BASE64_STANDARD};
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE, LOCATION, SET_COOKIE};
use reqwest::{multipart, redirect::Policy};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::cookies::CookieJar;
use super::types::{
    ExecError, ExecutionResult, Hop, PartData, ResolvedBody, ResolvedRequest, Sizes,
    TimingBreakdown,
};
use crate::model::Method;

/// Identifies a distinct `reqwest::Client` configuration worth caching:
/// clients are relatively expensive to build (connection pools, TLS
/// config) so we keep one per `(verify_tls, proxy, tls material)`
/// combination rather than building a fresh one per request. TLS material
/// is keyed by a content hash, so a rotated cert file yields a new client
/// without holding the PEM bytes in the key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientKey {
    verify_tls: bool,
    proxy: Option<String>,
    tls_fingerprint: u64,
    /// NTLM authenticates the TCP connection, so its clients are isolated
    /// (own pool, at most one idle connection) and pinned to HTTP/1.1 —
    /// HTTP/2 multiplexing would break the per-connection handshake.
    ntlm: bool,
}

fn tls_fingerprint(client_pem: Option<&[u8]>, extra_roots_pem: Option<&[u8]>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    client_pem.hash(&mut h);
    extra_roots_pem.hash(&mut h);
    h.finish()
}

/// Executes [`ResolvedRequest`]s over HTTP, owning a cookie jar and a small
/// pool of `reqwest::Client`s.
///
/// Redirects are followed manually (not by reqwest) so that each hop can be
/// recorded, cookies re-evaluated per hop, and `Authorization` stripped on
/// cross-origin hops.
pub struct HttpEngine {
    clients: Mutex<HashMap<ClientKey, reqwest::Client>>,
    cookies: CookieJar,
}

impl Default for HttpEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpEngine {
    pub fn new() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            cookies: CookieJar::new(),
        }
    }

    /// Handle to the engine's cookie jar, e.g. for a cookie-manager UI.
    pub fn cookies(&self) -> &CookieJar {
        &self.cookies
    }

    /// Execute `req`, following redirects manually, until a final response
    /// is reached, the overall `req.timeout` elapses, or `cancel` fires.
    pub async fn execute(
        &self,
        req: ResolvedRequest,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        let start_url =
            Url::parse(&req.url).map_err(|e| ExecError::InvalidUrl(format!("{e}: {}", req.url)))?;
        let client = self.client_for(
            req.verify_tls,
            req.proxy.as_deref(),
            req.client_pem.as_deref(),
            req.extra_roots_pem.as_deref(),
            req.ntlm.is_some(),
        )?;
        let timeout = req.timeout;

        match tokio::time::timeout(timeout, self.run(&req, start_url, client, cancel)).await {
            Ok(result) => result,
            Err(_elapsed) => Err(ExecError::Timeout(timeout)),
        }
    }

    fn client_for(
        &self,
        verify_tls: bool,
        proxy: Option<&str>,
        client_pem: Option<&[u8]>,
        extra_roots_pem: Option<&[u8]>,
        ntlm: bool,
    ) -> Result<reqwest::Client, ExecError> {
        let key = ClientKey {
            verify_tls,
            proxy: proxy.map(|s| s.to_string()),
            tls_fingerprint: tls_fingerprint(client_pem, extra_roots_pem),
            ntlm,
        };

        if let Some(client) = self.lock_clients().get(&key) {
            return Ok(client.clone());
        }

        // Redirects are handled manually by `run`, not by reqwest.
        // Per-request timeouts are set on each `RequestBuilder` in `run`
        // rather than here, because a single cached client is shared across
        // requests with different `timeout` values.
        let mut builder = reqwest::Client::builder()
            .redirect(Policy::none())
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .cookie_store(false);

        if key.ntlm {
            builder = builder.http1_only().pool_max_idle_per_host(1);
        }
        if !key.verify_tls {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(pem) = client_pem {
            let identity = reqwest::Identity::from_pem(pem)
                .map_err(|e| ExecError::Http(format!("invalid client certificate/key PEM: {e}")))?;
            builder = builder.identity(identity);
        }
        if let Some(pem) = extra_roots_pem {
            let certs = reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|e| ExecError::Http(format!("invalid CA bundle PEM: {e}")))?;
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }
        if let Some(proxy_url) = &key.proxy {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| ExecError::Http(format!("invalid proxy {proxy_url:?}: {e}")))?;
            builder = builder.proxy(proxy);
        }

        let client = builder
            .build()
            .map_err(|e| ExecError::Http(format!("failed to build HTTP client: {e}")))?;

        self.lock_clients().insert(key, client.clone());
        Ok(client)
    }

    fn lock_clients(&self) -> std::sync::MutexGuard<'_, HashMap<ClientKey, reqwest::Client>> {
        self.clients.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn run(
        &self,
        req: &ResolvedRequest,
        start_url: Url,
        client: reqwest::Client,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        let overall_start = Instant::now();
        let mut current_url = start_url;
        let mut current_method = req.method;
        let mut current_headers = req.headers.clone();
        let mut current_body = req.body.clone();
        let mut redirect_chain: Vec<Hop> = Vec::new();
        let mut hop_count: u32 = 0;
        let mut digest_answered = false;
        let mut ntlm_state = NtlmState::Fresh;

        loop {
            if cancel.is_cancelled() {
                return Err(ExecError::Cancelled);
            }

            let jar_pairs = self.cookies.matching(&current_url);
            let mut leg_headers = merge_cookie_header(&current_headers, &jar_pairs);
            if let Some(sigv4) = &req.sigv4 {
                apply_sigv4(&mut leg_headers, sigv4, current_method, &current_url, &current_body)?;
            }
            let header_map = header_map_from_pairs(&leg_headers)?;
            let request_bytes =
                approx_request_bytes(current_method, &current_url, &leg_headers, &current_body);

            let builder = client
                .request(to_reqwest_method(current_method), current_url.clone())
                .headers(header_map)
                .timeout(req.timeout);
            let builder = apply_body(builder, &current_body, &leg_headers, &cancel).await?;

            let leg_start = Instant::now();
            let response = race(builder.send(), &cancel)
                .await?
                .map_err(|e| map_reqwest_error(e, req.timeout))?;
            let ttfb = leg_start.elapsed();

            for raw in response.headers().get_all(SET_COOKIE) {
                if let Ok(s) = raw.to_str() {
                    self.cookies.store(&current_url, s);
                }
            }

            let status = response.status();

            // NTLM: Negotiate (type 1) → server Challenge (type 2) →
            // Authenticate (type 3), all riding the same keep-alive
            // connection (the client for NTLM requests is HTTP/1.1-only
            // with a single-connection pool).
            if status.as_u16() == 401 && ntlm_state != NtlmState::Done {
                if let Some(creds) = &req.ntlm {
                    let ntlm_challenge = response
                        .headers()
                        .get_all(reqwest::header::WWW_AUTHENTICATE)
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .find_map(|v| {
                            let v = v.trim_start();
                            v.strip_prefix("NTLM").map(|rest| rest.trim().to_string())
                        });
                    match (&ntlm_state, ntlm_challenge) {
                        (NtlmState::Fresh, Some(challenge)) if challenge.is_empty() => {
                            let negotiate = ntlm_negotiate(creds)?;
                            set_header(&mut current_headers, "Authorization", negotiate);
                            ntlm_state = NtlmState::Negotiated;
                            continue;
                        }
                        (NtlmState::Negotiated, Some(challenge)) if !challenge.is_empty() => {
                            let authenticate = ntlm_authenticate(creds, &challenge)?;
                            set_header(&mut current_headers, "Authorization", authenticate);
                            ntlm_state = NtlmState::Done;
                            continue;
                        }
                        // Anything else: not an NTLM exchange we can drive;
                        // fall through and surface the 401.
                        _ => {}
                    }
                }
            }

            // Digest auth: answer the server's 401 challenge once, then
            // retry the same request with the computed Authorization.
            if status.as_u16() == 401 && !digest_answered {
                if let Some(creds) = &req.digest {
                    let challenge = response
                        .headers()
                        .get_all(reqwest::header::WWW_AUTHENTICATE)
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .find(|v| v.trim_start().to_ascii_lowercase().starts_with("digest"));
                    if let Some(challenge) = challenge {
                        let authorization =
                            digest_authorization(challenge, creds, current_method, &current_url)?;
                        current_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
                        current_headers.push(("Authorization".to_string(), authorization));
                        digest_answered = true;
                        continue;
                    }
                }
            }

            let is_redirect_status = matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308);
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if is_redirect_status && req.follow_redirects && hop_count < req.max_redirects {
                if let Some(loc) = location {
                    redirect_chain.push(Hop {
                        status: status.as_u16(),
                        url: current_url.to_string(),
                        location: Some(loc.clone()),
                    });

                    let next_url = current_url.join(&loc).map_err(|e| {
                        ExecError::InvalidUrl(format!("invalid redirect location {loc:?}: {e}"))
                    })?;

                    let status_code = status.as_u16();
                    let switch_to_get = status_code == 303
                        || ((status_code == 301 || status_code == 302)
                            && current_method == Method::Post);
                    let (next_method, next_body) = if switch_to_get {
                        (Method::Get, ResolvedBody::None)
                    } else {
                        (current_method, current_body.clone())
                    };

                    if origin_of(&current_url) != origin_of(&next_url) {
                        // Cookie-jar cookies are already scoped per-hop by
                        // the jar itself; an explicit user-set `Cookie`
                        // header is not, so it must be stripped here too —
                        // same reasoning as `Authorization`.
                        current_headers.retain(|(k, _)| {
                            !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("cookie")
                        });
                    }

                    current_url = next_url;
                    current_method = next_method;
                    current_body = next_body;
                    hop_count += 1;
                    continue;
                }
                // Redirect status with no Location header: nothing to
                // follow, fall through and treat it as the final response.
            }

            let version = version_str(response.version());
            let status_text = status.canonical_reason().unwrap_or("").to_string();
            let response_headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
                .collect();
            let cookies_set: Vec<String> = response
                .headers()
                .get_all(SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok().map(|s| s.to_string()))
                .collect();
            let header_bytes: u64 = response_headers
                .iter()
                .map(|(k, v)| (k.len() + v.len() + 4) as u64)
                .sum();
            let effective_url = current_url.to_string();
            let status_code = status.as_u16();

            let body_start = Instant::now();
            let body = race(response.bytes(), &cancel)
                .await?
                .map_err(|e| map_reqwest_error(e, req.timeout))?;
            let download = body_start.elapsed();
            let total = overall_start.elapsed();

            return Ok(ExecutionResult {
                status: status_code,
                status_text,
                http_version: version.to_string(),
                headers: response_headers,
                body: body.to_vec(),
                timing: TimingBreakdown {
                    dns: None,
                    connect_tls: None,
                    connect: None,
                    tls: None,
                    ttfb,
                    download,
                    total,
                },
                size: Sizes {
                    request_bytes,
                    header_bytes,
                    body_bytes: body.len() as u64,
                },
                effective_url,
                redirect_chain,
                cookies_set,
                executed_at: Utc::now(),
            });
        }
    }
}

/// Where the NTLM handshake currently stands for this request.
#[derive(Debug, PartialEq, Eq)]
enum NtlmState {
    Fresh,
    Negotiated,
    Done,
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value));
}

fn ntlm_creds(creds: &super::types::NtlmCredentials) -> ntlmclient::Credentials {
    ntlmclient::Credentials {
        username: creds.username.clone(),
        password: creds.password.clone(),
        domain: creds.domain.clone(),
    }
}

/// Build the NTLM Negotiate (type 1) Authorization header.
fn ntlm_negotiate(creds: &super::types::NtlmCredentials) -> Result<String, ExecError> {
    let message = ntlmclient::Message::Negotiate(ntlmclient::NegotiateMessage {
        flags: ntlmclient::Flags::NEGOTIATE_UNICODE
            | ntlmclient::Flags::REQUEST_TARGET
            | ntlmclient::Flags::NEGOTIATE_NTLM
            | ntlmclient::Flags::NEGOTIATE_NTLM2_KEY
            | ntlmclient::Flags::NEGOTIATE_ALWAYS_SIGN,
        supplied_domain: creds.domain.clone(),
        supplied_workstation: String::new(),
        os_version: ntlmclient::OsVersion::default(),
    });
    let bytes = message
        .to_bytes()
        .map_err(|e| ExecError::Http(format!("NTLM negotiate failed: {e:?}")))?;
    Ok(format!("NTLM {}", BASE64_STANDARD.encode(bytes)))
}

/// Answer an NTLM Challenge (type 2) with an NTLMv2 Authenticate (type 3)
/// Authorization header.
fn ntlm_authenticate(
    creds: &super::types::NtlmCredentials,
    challenge_b64: &str,
) -> Result<String, ExecError> {
    let challenge_bytes = BASE64_STANDARD
        .decode(challenge_b64)
        .map_err(|e| ExecError::Http(format!("invalid NTLM challenge encoding: {e}")))?;
    let message = ntlmclient::Message::try_from(challenge_bytes.as_slice())
        .map_err(|e| ExecError::Http(format!("invalid NTLM challenge: {e:?}")))?;
    let ntlmclient::Message::Challenge(challenge) = message else {
        return Err(ExecError::Http("server did not send an NTLM challenge".to_string()));
    };

    let target_info: Vec<u8> =
        challenge.target_information.iter().flat_map(|entry| entry.to_bytes()).collect();
    // NTLMv2 timestamps are Windows FILETIME: 100ns ticks since 1601-01-01.
    let now_filetime = (Utc::now().timestamp() + 11_644_473_600) * 10_000_000;
    let response = ntlmclient::respond_challenge_ntlm_v2(
        challenge.challenge,
        &target_info,
        now_filetime,
        &ntlm_creds(creds),
    );

    let authenticate = ntlmclient::Message::Authenticate(ntlmclient::AuthenticateMessage {
        lm_response: response.lm_response,
        ntlm_response: response.ntlm_response,
        domain_name: creds.domain.clone(),
        user_name: creds.username.clone(),
        workstation_name: String::new(),
        session_key: Vec::new(),
        flags: ntlmclient::Flags::NEGOTIATE_UNICODE | ntlmclient::Flags::NEGOTIATE_NTLM,
        os_version: ntlmclient::OsVersion::default(),
    });
    let bytes = authenticate
        .to_bytes()
        .map_err(|e| ExecError::Http(format!("NTLM authenticate failed: {e:?}")))?;
    Ok(format!("NTLM {}", BASE64_STANDARD.encode(bytes)))
}

/// Compute the `Authorization` header answering a Digest challenge
/// (RFC 7616, qop=auth; auth-int is not supported).
fn digest_authorization(
    challenge: &str,
    creds: &super::types::DigestCredentials,
    method: Method,
    url: &Url,
) -> Result<String, ExecError> {
    let mut prompt = digest_auth::parse(challenge)
        .map_err(|e| ExecError::Http(format!("invalid Digest challenge: {e}")))?;
    let uri = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let context = digest_auth::AuthContext::new_with_method(
        creds.username.clone(),
        creds.password.clone(),
        uri,
        Option::<&[u8]>::None,
        digest_auth::HttpMethod::from(method.as_str()),
    );
    let answer = prompt
        .respond(&context)
        .map_err(|e| ExecError::Http(format!("failed to answer Digest challenge: {e}")))?;
    Ok(answer.to_header_string())
}

/// Sign this hop with AWS SigV4, appending `x-amz-date`, the optional
/// session-token header and `Authorization` to `headers`.
fn apply_sigv4(
    headers: &mut Vec<(String, String)>,
    params: &super::types::SigV4Params,
    method: Method,
    url: &Url,
    body: &ResolvedBody,
) -> Result<(), ExecError> {
    use aws_sigv4::http_request::{sign, SignableBody, SignableRequest, SigningSettings};
    use aws_sigv4::sign::v4;

    let identity: aws_smithy_runtime_api::client::identity::Identity =
        aws_credential_types::Credentials::new(
            params.access_key.clone(),
            params.secret_key.clone(),
            params.session_token.clone(),
            None,
            "forge",
        )
        .into();

    let signing_params: aws_sigv4::http_request::SigningParams<'_> = v4::SigningParams::builder()
        .identity(&identity)
        .region(&params.region)
        .name(&params.service)
        .time(std::time::SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .map_err(|e| ExecError::Http(format!("invalid SigV4 parameters: {e}")))?
        .into();

    // SigV4 requires a Host header in the canonical request; reqwest adds
    // it on the wire, so mirror it here for signing.
    let host = url.host_str().unwrap_or_default().to_string();
    let host_header = match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    };
    let mut to_sign: Vec<(String, String)> = headers.clone();
    if !to_sign.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        to_sign.push(("host".to_string(), host_header));
    }

    let form_body;
    let signable_body = match body {
        ResolvedBody::None => SignableBody::Bytes(b""),
        ResolvedBody::Bytes { data, .. } => SignableBody::Bytes(data),
        ResolvedBody::Form(pairs) => {
            form_body = form_urlencode(pairs);
            SignableBody::Bytes(form_body.as_bytes())
        }
        // Multipart bodies are streamed with generated boundaries; sign
        // them as unsigned payload (the SigV4-sanctioned escape hatch).
        ResolvedBody::Multipart(_) => SignableBody::UnsignedPayload,
    };

    let signable = SignableRequest::new(
        method.as_str(),
        url.as_str(),
        to_sign.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        signable_body,
    )
    .map_err(|e| ExecError::Http(format!("SigV4: unsignable request: {e}")))?;

    let (instructions, _signature) = sign(signable, &signing_params)
        .map_err(|e| ExecError::Http(format!("SigV4 signing failed: {e}")))?
        .into_parts();
    for header in instructions.into_parts().0 {
        headers.retain(|(k, _)| !k.eq_ignore_ascii_case(header.name()));
        headers.push((header.name().to_string(), header.value().to_string()));
    }
    Ok(())
}

/// Race `fut` against cancellation, so long-running sends/reads/file-reads
/// can be aborted promptly rather than only at the outer timeout.
async fn race<F, T>(fut: F, cancel: &CancellationToken) -> Result<T, ExecError>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ExecError::Cancelled),
        v = fut => Ok(v),
    }
}

fn map_reqwest_error(e: reqwest::Error, timeout: Duration) -> ExecError {
    if e.is_timeout() {
        ExecError::Timeout(timeout)
    } else if e.is_connect() {
        ExecError::Connect(e.to_string())
    } else {
        ExecError::Http(e.to_string())
    }
}

fn to_reqwest_method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Patch => reqwest::Method::PATCH,
        Method::Delete => reqwest::Method::DELETE,
        Method::Head => reqwest::Method::HEAD,
        Method::Options => reqwest::Method::OPTIONS,
        Method::Trace => reqwest::Method::TRACE,
    }
}

fn version_str(v: reqwest::Version) -> &'static str {
    if v == reqwest::Version::HTTP_09 {
        "HTTP/0.9"
    } else if v == reqwest::Version::HTTP_10 {
        "HTTP/1.0"
    } else if v == reqwest::Version::HTTP_11 {
        "HTTP/1.1"
    } else if v == reqwest::Version::HTTP_2 {
        "HTTP/2"
    } else if v == reqwest::Version::HTTP_3 {
        "HTTP/3"
    } else {
        "HTTP/1.1"
    }
}

/// `(scheme, host, port)` — used to decide whether a redirect crosses an
/// origin boundary (and should therefore drop `Authorization`).
fn origin_of(url: &Url) -> (String, String, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().unwrap_or("").to_ascii_lowercase(),
        url.port_or_known_default(),
    )
}

/// Merge cookie-jar matches into the request's `Cookie` header, preserving
/// any cookies the caller already set explicitly (those win on name
/// collision) and the header's original position in the list.
fn merge_cookie_header(
    user_headers: &[(String, String)],
    jar_pairs: &[(String, String)],
) -> Vec<(String, String)> {
    if jar_pairs.is_empty() {
        return user_headers.to_vec();
    }

    let mut seen = std::collections::HashSet::new();
    let mut cookie_parts: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut cookie_slot: Option<usize> = None;

    for (k, v) in user_headers {
        if k.eq_ignore_ascii_case("cookie") {
            if cookie_slot.is_none() {
                cookie_slot = Some(out.len());
            }
            for part in v.split(';') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                let name = part.split('=').next().unwrap_or(part).trim();
                if seen.insert(name.to_ascii_lowercase()) {
                    cookie_parts.push(part.to_string());
                }
            }
            continue;
        }
        out.push((k.clone(), v.clone()));
    }

    for (name, value) in jar_pairs {
        if seen.insert(name.to_ascii_lowercase()) {
            cookie_parts.push(format!("{name}={value}"));
        }
    }

    let merged = cookie_parts.join("; ");
    let idx = cookie_slot.unwrap_or(out.len()).min(out.len());
    out.insert(idx, ("Cookie".to_string(), merged));
    out
}

fn header_map_from_pairs(pairs: &[(String, String)]) -> Result<HeaderMap, ExecError> {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| ExecError::Http(format!("invalid header name {k:?}: {e}")))?;
        let value = HeaderValue::from_str(v)
            .map_err(|e| ExecError::Http(format!("invalid header value for {k:?}: {e}")))?;
        map.append(name, value);
    }
    Ok(map)
}

async fn apply_body(
    builder: reqwest::RequestBuilder,
    body: &ResolvedBody,
    headers: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<reqwest::RequestBuilder, ExecError> {
    match body {
        ResolvedBody::None => Ok(builder),
        ResolvedBody::Bytes { content_type, data } => {
            let has_content_type = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
            let builder = if !has_content_type {
                if let Some(ct) = content_type {
                    builder.header(CONTENT_TYPE, ct.as_str())
                } else {
                    builder
                }
            } else {
                builder
            };
            Ok(builder.body(data.clone()))
        }
        ResolvedBody::Form(pairs) => {
            // The `form` reqwest feature isn't enabled in this workspace,
            // so the `application/x-www-form-urlencoded` body is built by
            // hand instead of via `RequestBuilder::form`.
            let has_content_type = headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
            let builder = if has_content_type {
                builder
            } else {
                builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            };
            Ok(builder.body(form_urlencode(pairs)))
        }
        ResolvedBody::Multipart(parts) => {
            let mut form = multipart::Form::new();
            for part in parts {
                let mut mp = match &part.data {
                    PartData::Text(text) => multipart::Part::text(text.clone()),
                    PartData::File(path) => {
                        let bytes = race(tokio::fs::read(path), cancel).await?.map_err(|e| {
                            ExecError::BodyFile {
                                path: path.display().to_string(),
                                message: e.to_string(),
                            }
                        })?;
                        multipart::Part::bytes(bytes)
                    }
                };
                if let Some(file_name) = &part.file_name {
                    mp = mp.file_name(file_name.clone());
                }
                if let Some(ct) = &part.content_type {
                    mp = mp
                        .mime_str(ct)
                        .map_err(|e| ExecError::Http(format!("invalid mime type {ct:?}: {e}")))?;
                }
                form = form.part(part.name.clone(), mp);
            }
            Ok(builder.multipart(form))
        }
    }
}

fn approx_request_bytes(
    method: Method,
    url: &Url,
    headers: &[(String, String)],
    body: &ResolvedBody,
) -> u64 {
    let request_line = method.as_str().len() + 1 + url.as_str().len() + " HTTP/1.1\r\n".len();
    let headers_len: usize = headers.iter().map(|(k, v)| k.len() + 2 + v.len() + 2).sum();
    let body_len = match body {
        ResolvedBody::None => 0,
        ResolvedBody::Bytes { data, .. } => data.len(),
        ResolvedBody::Form(pairs) => form_len(pairs),
        ResolvedBody::Multipart(parts) => parts
            .iter()
            .map(|p| match &p.data {
                PartData::Text(t) => t.len(),
                PartData::File(path) => std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0),
            })
            .sum(),
    };
    (request_line + headers_len + body_len) as u64
}

/// Percent-encode a single `application/x-www-form-urlencoded` component:
/// unreserved characters pass through, spaces become `+`, everything else
/// is escaped as `%XX` (per byte, so multi-byte UTF-8 is handled correctly).
fn form_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Encode `pairs` as an `application/x-www-form-urlencoded` body.
fn form_urlencode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode_component(k), form_encode_component(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Rough (unencoded) approximation of an `application/x-www-form-urlencoded`
/// body's length.
fn form_len(pairs: &[(String, String)]) -> usize {
    let mut total = 0usize;
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            total += 1; // '&'
        }
        total += k.len() + 1 + v.len(); // 'k=v'
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_jar_cookies_with_existing_header() {
        let user = vec![("Cookie".to_string(), "a=1".to_string())];
        let jar = vec![("b".to_string(), "2".to_string()), ("a".to_string(), "override".to_string())];
        let merged = merge_cookie_header(&user, &jar);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0, "Cookie");
        // User-provided `a=1` wins over the jar's `a=override`.
        assert!(merged[0].1.contains("a=1"));
        assert!(merged[0].1.contains("b=2"));
    }

    #[test]
    fn no_jar_cookies_leaves_headers_untouched() {
        let user = vec![("X-Foo".to_string(), "bar".to_string())];
        let merged = merge_cookie_header(&user, &[]);
        assert_eq!(merged, user);
    }

    #[test]
    fn origin_differs_on_port() {
        let a = Url::parse("http://example.com:8080/x").unwrap();
        let b = Url::parse("http://example.com:8081/x").unwrap();
        assert_ne!(origin_of(&a), origin_of(&b));
    }

    #[test]
    fn origin_same_for_default_port() {
        let a = Url::parse("http://example.com/x").unwrap();
        let b = Url::parse("http://example.com:80/y").unwrap();
        assert_eq!(origin_of(&a), origin_of(&b));
    }

    #[test]
    fn approx_request_bytes_grows_with_body() {
        let url = Url::parse("http://example.com/").unwrap();
        let empty = approx_request_bytes(Method::Get, &url, &[], &ResolvedBody::None);
        let with_body = approx_request_bytes(
            Method::Post,
            &url,
            &[],
            &ResolvedBody::Bytes {
                content_type: None,
                data: vec![0u8; 100],
            },
        );
        assert!(with_body > empty);
    }

    #[test]
    fn version_str_maps_known_versions() {
        assert_eq!(version_str(reqwest::Version::HTTP_11), "HTTP/1.1");
        assert_eq!(version_str(reqwest::Version::HTTP_2), "HTTP/2");
    }
}
