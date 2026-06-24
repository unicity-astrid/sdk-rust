//! HTTP client with SSRF protection and streaming.
//!
//! Backed by `astrid:http/host@1.1.0`. The buffered path
//! ([`send`]) returns the full response in memory (10 MB default cap);
//! the streaming path ([`stream_start`]) returns a RAII [`HttpStream`]
//! that drains response chunks until EOF and drops the kernel-side
//! resource automatically.
//!
//! Every request can carry per-request controls — timeouts, redirect
//! policy, size / decompression caps, scheme restriction, subresource
//! integrity — set through the [`Request`] fluent builders. An unset
//! option reproduces `astrid:http@1.0.0` behaviour exactly: the host
//! applies its default for that field.

use super::*;
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

/// How the host treats a `3xx` redirect response.
///
/// Maps to the WIT `astrid:http/host.redirect-policy`. When a redirect
/// is followed, every hop is re-validated through the SSRF airlock and
/// `Authorization` / `Cookie` are stripped on a cross-origin hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum RedirectPolicy {
    /// Follow redirects (bounded by [`Request::max_redirects`]).
    Follow,
    /// Do not follow; fail with a `redirect-blocked` host error.
    Error,
    /// Do not follow; return the `3xx` response to the caller as-is.
    Manual,
}

impl RedirectPolicy {
    fn to_wit(self) -> wit_http::RedirectPolicy {
        match self {
            Self::Follow => wit_http::RedirectPolicy::Follow,
            Self::Error => wit_http::RedirectPolicy::Error,
            Self::Manual => wit_http::RedirectPolicy::Manual,
        }
    }
}

/// Convert a [`Duration`] to whole milliseconds for a WIT
/// `timeout-config` field, saturating at [`u64::MAX`] rather than
/// wrapping (an absurdly long timeout still clamps to the host ceiling).
fn duration_to_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// An HTTP request.
///
/// Construct via [`Request::get`], [`Request::post`], etc. or
/// [`Request::new`] for arbitrary methods. Bodies can be raw bytes
/// (see [`Request::body_bytes`]) or a UTF-8 string (see
/// [`Request::body`]).
///
/// Per-request controls (timeouts, redirects, size / decompression
/// caps, scheme restriction, integrity) are set through the fluent
/// builder methods below. Every control is optional; an unset control
/// selects the host default for that field, reproducing
/// `astrid:http@1.0.0` behaviour.
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    url: String,
    method: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Vec<u8>>,

    // Per-request controls (@1.1.0 `request-options`). Each `None`
    // selects the host default for that field.
    #[serde(skip_serializing_if = "Option::is_none")]
    connect_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_byte_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_timeout: Option<Duration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect: Option<RedirectPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_redirects: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_decompressed_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_decompress: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    https_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    integrity: Option<String>,
}

impl Request {
    /// Create a request with an arbitrary method.
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: method.into(),
            headers: HashMap::new(),
            body: None,
            connect_timeout: None,
            first_byte_timeout: None,
            read_timeout: None,
            total_timeout: None,
            redirect: None,
            max_redirects: None,
            max_response_bytes: None,
            max_decompressed_bytes: None,
            auto_decompress: None,
            https_only: None,
            integrity: None,
        }
    }

    /// Create a GET request.
    pub fn get(url: impl Into<String>) -> Self {
        Self::new("GET", url)
    }

    /// Create a POST request.
    pub fn post(url: impl Into<String>) -> Self {
        Self::new("POST", url)
    }

    /// Create a PUT request.
    pub fn put(url: impl Into<String>) -> Self {
        Self::new("PUT", url)
    }

    /// Create a DELETE request.
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new("DELETE", url)
    }

    /// Add a header.
    #[must_use]
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the request body as a UTF-8 string.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into().into_bytes());
        self
    }

    /// Set the request body as raw bytes (image/binary uploads).
    #[must_use]
    pub fn body_bytes(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Set a JSON body (serializes the value and sets Content-Type).
    pub fn json<T: Serialize>(self, value: &T) -> Result<Self, SysError> {
        let json = serde_json::to_string(value)?;
        Ok(self.header("Content-Type", "application/json").body(json))
    }

    // ---- Per-request controls (@1.1.0) -------------------------------

    /// Set the whole-request hard deadline (connect + send + receive).
    ///
    /// A default the host may clamp to its own ceiling. Unset selects
    /// the host default (30 s on the buffered path).
    #[must_use]
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.total_timeout = Some(dur);
        self
    }

    /// Set the TCP + TLS connection-establishment deadline.
    ///
    /// A default; the host may clamp it. Unset selects the host default.
    #[must_use]
    pub fn connect_timeout(mut self, dur: Duration) -> Self {
        self.connect_timeout = Some(dur);
        self
    }

    /// Set the deadline from request fully sent to first response byte
    /// (catches an accept-then-hang server).
    ///
    /// A default; the host may clamp it. Unset selects the host default.
    #[must_use]
    pub fn first_byte_timeout(mut self, dur: Duration) -> Self {
        self.first_byte_timeout = Some(dur);
        self
    }

    /// Set the maximum idle gap between successive body chunks on the
    /// streaming path (the between-bytes / per-chunk timeout).
    ///
    /// A default; the host may clamp it. Unset selects the host default
    /// (120 s per chunk).
    #[must_use]
    pub fn read_timeout(mut self, dur: Duration) -> Self {
        self.read_timeout = Some(dur);
        self
    }

    /// Set how the host treats a `3xx` redirect.
    ///
    /// A ceiling the host clamps: the host default for an untrusted
    /// capsule is [`RedirectPolicy::Error`] — never a silent follow.
    #[must_use]
    pub fn redirect(mut self, policy: RedirectPolicy) -> Self {
        self.redirect = Some(policy);
        self
    }

    /// Set the redirect hop limit (applies under
    /// [`RedirectPolicy::Follow`]).
    ///
    /// A ceiling the host clamps to its own maximum. Unset selects the
    /// host default.
    #[must_use]
    pub fn max_redirects(mut self, max: u32) -> Self {
        self.max_redirects = Some(max);
        self
    }

    /// Cap the buffered response body in bytes.
    ///
    /// A ceiling the host clamps: the host enforces a hard maximum above
    /// which `body-too-large` is returned regardless of this value.
    /// Unset selects the host 10 MB default.
    #[must_use]
    pub fn max_response_bytes(mut self, max: u64) -> Self {
        self.max_response_bytes = Some(max);
        self
    }

    /// Cap the DECOMPRESSED body in bytes (decompression-bomb defence).
    ///
    /// A ceiling the host clamps to its own maximum. Unset selects the
    /// host default ceiling.
    #[must_use]
    pub fn max_decompressed_bytes(mut self, max: u64) -> Self {
        self.max_decompressed_bytes = Some(max);
        self
    }

    /// Toggle automatic gzip / brotli / deflate / zstd decompression.
    ///
    /// `true` (the host default) decodes the body; `false` delivers the
    /// raw wire bytes — required for binary or already-compressed
    /// downloads. Unset selects the host default (`true`).
    #[must_use]
    pub fn auto_decompress(mut self, on: bool) -> Self {
        self.auto_decompress = Some(on);
        self
    }

    /// Refuse any non-`https` scheme with a `scheme-denied` host error.
    ///
    /// A ceiling: the host rejects the request before any connection.
    #[must_use]
    pub fn https_only(mut self) -> Self {
        self.https_only = Some(true);
        self
    }

    /// Require the response body to match a subresource-integrity digest.
    ///
    /// Formatted `sha256-<base64>` (also `sha384-` / `sha512-`). The
    /// host verifies the body against the digest and returns an
    /// `integrity-mismatch` host error on failure.
    #[must_use]
    pub fn integrity(mut self, digest: impl Into<String>) -> Self {
        self.integrity = Some(digest.into());
        self
    }

    /// Convert to the WIT `http-request-data` type (shape unchanged from
    /// @1.0.0 — per-request behaviour rides in [`Self::to_options`]).
    fn to_wit(&self) -> wit_http::HttpRequestData {
        wit_http::HttpRequestData {
            url: self.url.clone(),
            method: parse_method(&self.method),
            headers: self
                .headers
                .iter()
                .map(|(k, v)| wit_http::KeyValuePair {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect(),
            body: self.body.clone(),
        }
    }

    /// Build the WIT `request-options` from the stored per-request
    /// controls. An all-unset `Request` yields an all-`None`
    /// `request-options`, which the host treats as @1.0.0 defaults.
    fn to_options(&self) -> wit_http::RequestOptions {
        let timeouts = if self.connect_timeout.is_some()
            || self.first_byte_timeout.is_some()
            || self.read_timeout.is_some()
            || self.total_timeout.is_some()
        {
            Some(wit_http::TimeoutConfig {
                connect_ms: self.connect_timeout.map(duration_to_ms),
                first_byte_ms: self.first_byte_timeout.map(duration_to_ms),
                between_bytes_ms: self.read_timeout.map(duration_to_ms),
                total_ms: self.total_timeout.map(duration_to_ms),
            })
        } else {
            None
        };

        wit_http::RequestOptions {
            timeouts,
            redirect: self.redirect.map(RedirectPolicy::to_wit),
            max_redirects: self.max_redirects,
            max_response_bytes: self.max_response_bytes,
            max_decompressed_bytes: self.max_decompressed_bytes,
            auto_decompress: self.auto_decompress,
            https_only: self.https_only,
            integrity: self.integrity.clone(),
        }
    }
}

/// Parse a textual HTTP method to the typed
/// [`astrid_sys::astrid::http::host::HttpMethod`] variant. Non-standard
/// methods fall through to `Other(String)`.
fn parse_method(method: &str) -> wit_http::HttpMethod {
    match method.to_ascii_uppercase().as_str() {
        "GET" => wit_http::HttpMethod::Get,
        "HEAD" => wit_http::HttpMethod::Head,
        "POST" => wit_http::HttpMethod::Post,
        "PUT" => wit_http::HttpMethod::Put,
        "DELETE" => wit_http::HttpMethod::Delete,
        "CONNECT" => wit_http::HttpMethod::Connect,
        "OPTIONS" => wit_http::HttpMethod::Options,
        "TRACE" => wit_http::HttpMethod::Trace,
        "PATCH" => wit_http::HttpMethod::Patch,
        other => wit_http::HttpMethod::Other(other.to_string()),
    }
}

/// Post-request metadata carried on a buffered [`Response`] (@1.1.0).
///
/// Constructed from the WIT `response-meta`. Read through the
/// [`Response`] accessors ([`Response::final_url`],
/// [`Response::redirect_count`], [`Response::elapsed`],
/// [`Response::wire_bytes`]).
#[derive(Debug, Clone)]
struct ResponseMeta {
    final_url: String,
    redirect_count: u32,
    elapsed_ms: u64,
    wire_bytes: u64,
}

impl From<wit_http::ResponseMeta> for ResponseMeta {
    fn from(m: wit_http::ResponseMeta) -> Self {
        Self {
            final_url: m.final_url,
            redirect_count: m.redirect_count,
            elapsed_ms: m.elapsed_ms,
            wire_bytes: m.wire_bytes,
        }
    }
}

/// An HTTP response from a non-streaming request.
///
/// All fields are private — use accessor methods to read them.
#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    meta: ResponseMeta,
}

impl Response {
    /// HTTP status code (e.g. 200, 404, 500).
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Response headers.
    #[must_use]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// The raw response body as bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// The response body as a UTF-8 string.
    pub fn text(&self) -> Result<&str, SysError> {
        core::str::from_utf8(&self.body).map_err(|e| SysError::ApiError(e.to_string()))
    }

    /// Deserialize the response body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, SysError> {
        serde_json::from_slice(&self.body).map_err(SysError::from)
    }

    /// Whether the status code indicates success (2xx).
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The final URL after any redirects were followed.
    ///
    /// Equals the request URL when no redirect occurred. New in @1.1.0.
    #[must_use]
    pub fn final_url(&self) -> &str {
        &self.meta.final_url
    }

    /// The number of redirect hops the host followed. New in @1.1.0.
    #[must_use]
    pub fn redirect_count(&self) -> u32 {
        self.meta.redirect_count
    }

    /// Wall-clock duration of the request (best-effort). New in @1.1.0.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.meta.elapsed_ms)
    }

    /// Bytes received on the wire, before decompression (best-effort).
    /// New in @1.1.0.
    #[must_use]
    pub fn wire_bytes(&self) -> u64 {
        self.meta.wire_bytes
    }
}

/// Send an HTTP request and wait for the full response.
///
/// Default 30 s timeout and 10 MB max response body unless overridden
/// via the [`Request`] builders. For larger / chunked bodies use
/// [`stream_start`].
pub fn send(request: &Request) -> Result<Response, SysError> {
    let wit_req = request.to_wit();
    let opts = request.to_options();
    let result = wit_http::http_request_opts(&wit_req, &opts).map_err(host_err)?;
    let headers: HashMap<String, String> = result
        .headers
        .into_iter()
        .map(|kv| (kv.key, kv.value))
        .collect();
    Ok(Response {
        status: result.status,
        headers,
        body: result.body,
        meta: result.meta.into(),
    })
}

/// An active streaming HTTP response.
///
/// Owns the kernel-side `http-stream` resource. Drop closes the stream
/// automatically — capsules don't need to call `close` explicitly.
/// Per-capsule cap: 4 concurrent HTTP streams.
//
// TODO(http@1.1.0): the host `http-stream.trailers` accessor is not yet
// surfaced here — the host returns `none` (no trailers) for now, so a
// trailers() wrapper would be a non-functional API. Add it once the host
// populates trailers.
#[derive(Debug)]
pub struct HttpStream {
    inner: wit_http::HttpStream,
    status: u16,
    headers: HashMap<String, String>,
}

impl HttpStream {
    /// HTTP status code returned at stream start.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Headers from the initial response.
    #[must_use]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// Read the next chunk. Returns `Ok(None)` at EOF; callers loop
    /// until `None`.
    pub fn read_chunk(&self) -> Result<Option<Vec<u8>>, SysError> {
        let bytes = self.inner.read_chunk().map_err(host_err)?;
        if bytes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(bytes))
        }
    }

    /// Close the stream explicitly. Idempotent. Equivalent to dropping
    /// the resource.
    pub fn close(self) -> Result<(), SysError> {
        self.inner.close().map_err(host_err)
    }
}

/// Start a streaming HTTP request.
///
/// Sends the request and waits for the status/headers to arrive.
/// Returns an [`HttpStream`] from which the body is read in chunks.
/// Per-request controls set on the [`Request`] (the between-bytes
/// timeout, redirect policy, scheme restriction, etc.) apply.
//
// TODO(http@1.1.0): the host also exposes `http-upload-start` for a
// streaming request body (upload). It is not surfaced here — the host
// returns not-implemented for now. Add the upload path once the host
// implements it.
pub fn stream_start(request: &Request) -> Result<HttpStream, SysError> {
    let wit_req = request.to_wit();
    let opts = request.to_options();
    let stream = wit_http::http_stream_start_opts(&wit_req, &opts).map_err(host_err)?;
    let status = stream.status();
    let headers: HashMap<String, String> = stream
        .headers()
        .into_iter()
        .map(|kv| (kv.key, kv.value))
        .collect();
    Ok(HttpStream {
        inner: stream,
        status,
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_to_ms_converts_whole_millis() {
        assert_eq!(duration_to_ms(Duration::from_millis(250)), 250);
        assert_eq!(duration_to_ms(Duration::from_secs(30)), 30_000);
        assert_eq!(duration_to_ms(Duration::ZERO), 0);
    }

    #[test]
    fn duration_to_ms_saturates_at_u64_max() {
        // A Duration whose millis exceed u64::MAX must clamp, not wrap.
        let huge = Duration::new(u64::MAX, 0);
        assert_eq!(duration_to_ms(huge), u64::MAX);
    }

    #[test]
    fn redirect_policy_maps_to_wit() {
        assert!(matches!(
            RedirectPolicy::Follow.to_wit(),
            wit_http::RedirectPolicy::Follow
        ));
        assert!(matches!(
            RedirectPolicy::Error.to_wit(),
            wit_http::RedirectPolicy::Error
        ));
        assert!(matches!(
            RedirectPolicy::Manual.to_wit(),
            wit_http::RedirectPolicy::Manual
        ));
    }

    #[test]
    fn unset_request_builds_empty_options() {
        // An untouched request must produce all-None options so the host
        // applies @1.0.0 defaults across the board.
        let req = Request::get("https://example.com");
        let opts = req.to_options();
        assert!(opts.timeouts.is_none());
        assert!(opts.redirect.is_none());
        assert!(opts.max_redirects.is_none());
        assert!(opts.max_response_bytes.is_none());
        assert!(opts.max_decompressed_bytes.is_none());
        assert!(opts.auto_decompress.is_none());
        assert!(opts.https_only.is_none());
        assert!(opts.integrity.is_none());
    }

    #[test]
    fn timeout_builders_populate_timeout_config() {
        let req = Request::get("https://example.com")
            .connect_timeout(Duration::from_millis(100))
            .first_byte_timeout(Duration::from_millis(200))
            .read_timeout(Duration::from_millis(300))
            .timeout(Duration::from_secs(5));
        let opts = req.to_options();
        let t = opts.timeouts.expect("timeouts populated");
        assert_eq!(t.connect_ms, Some(100));
        assert_eq!(t.first_byte_ms, Some(200));
        assert_eq!(t.between_bytes_ms, Some(300));
        assert_eq!(t.total_ms, Some(5_000));
    }

    #[test]
    fn partial_timeouts_leave_other_fields_none() {
        let req = Request::get("https://example.com").timeout(Duration::from_secs(2));
        let t = req.to_options().timeouts.expect("timeouts populated");
        assert_eq!(t.total_ms, Some(2_000));
        assert!(t.connect_ms.is_none());
        assert!(t.first_byte_ms.is_none());
        assert!(t.between_bytes_ms.is_none());
    }

    #[test]
    fn control_builders_populate_request_options() {
        let req = Request::get("https://example.com")
            .redirect(RedirectPolicy::Follow)
            .max_redirects(7)
            .max_response_bytes(1024)
            .max_decompressed_bytes(2048)
            .auto_decompress(false)
            .https_only()
            .integrity("sha256-abc");
        let opts = req.to_options();
        assert!(matches!(
            opts.redirect,
            Some(wit_http::RedirectPolicy::Follow)
        ));
        assert_eq!(opts.max_redirects, Some(7));
        assert_eq!(opts.max_response_bytes, Some(1024));
        assert_eq!(opts.max_decompressed_bytes, Some(2048));
        assert_eq!(opts.auto_decompress, Some(false));
        assert_eq!(opts.https_only, Some(true));
        assert_eq!(opts.integrity.as_deref(), Some("sha256-abc"));
    }
}
