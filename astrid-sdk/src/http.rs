//! HTTP client with SSRF protection and streaming.
//!
//! Backed by `astrid:http/host@1.0.0`. The buffered path
//! ([`send`]) returns the full response in memory (10 MB cap); the
//! streaming path ([`stream_start`]) returns a RAII [`HttpStream`]
//! that drains response chunks until EOF and drops the kernel-side
//! resource automatically.

use super::*;
use serde::Serialize;
use std::collections::HashMap;

/// An HTTP request.
///
/// Construct via [`Request::get`], [`Request::post`], etc. or
/// [`Request::new`] for arbitrary methods. Bodies can be raw bytes
/// (see [`Request::body_bytes`]) or a UTF-8 string (see
/// [`Request::body`]).
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    url: String,
    method: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<Vec<u8>>,
}

impl Request {
    /// Create a request with an arbitrary method.
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            method: method.into(),
            headers: HashMap::new(),
            body: None,
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

    /// Convert to the WIT `HttpRequestData` type.
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

/// An HTTP response from a non-streaming request.
///
/// All fields are private — use accessor methods to read them.
#[derive(Debug)]
pub struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
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
}

/// Send an HTTP request and wait for the full response.
///
/// 30 s timeout, 10 MB max response body. For larger / chunked bodies
/// use [`stream_start`].
pub fn send(request: &Request) -> Result<Response, SysError> {
    let wit_req = request.to_wit();
    let result = wit_http::http_request(&wit_req).map_err(host_err)?;
    let headers: HashMap<String, String> = result
        .headers
        .into_iter()
        .map(|kv| (kv.key, kv.value))
        .collect();
    Ok(Response {
        status: result.status,
        headers,
        body: result.body,
    })
}

/// An active streaming HTTP response.
///
/// Owns the kernel-side `http-stream` resource. Drop closes the stream
/// automatically — capsules don't need to call `close` explicitly.
/// Per-capsule cap: 4 concurrent HTTP streams.
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
pub fn stream_start(request: &Request) -> Result<HttpStream, SysError> {
    let wit_req = request.to_wit();
    let stream = wit_http::http_stream_start(&wit_req).map_err(host_err)?;
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
