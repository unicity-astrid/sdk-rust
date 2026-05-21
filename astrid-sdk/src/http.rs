//\! HTTP client with SSRF protection and streaming.

use super::*;
use serde::Serialize;
use std::collections::HashMap;

/// An HTTP request.
///
/// Construct via [`Request::get`], [`Request::post`], etc. or
/// [`Request::new`] for arbitrary methods.
#[derive(Debug, Clone, Serialize)]
pub struct Request {
    url: String,
    method: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
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
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Set the request body.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Set a JSON body (serializes the value and sets Content-Type).
    pub fn json<T: Serialize>(self, value: &T) -> Result<Self, SysError> {
        let json = serde_json::to_string(value)?;
        Ok(self.header("Content-Type", "application/json").body(json))
    }

    /// Convert to the WIT HttpRequestData type.
    fn to_wit(&self) -> wit_types::HttpRequestData {
        wit_types::HttpRequestData {
            url: self.url.clone(),
            method: self.method.clone(),
            headers: self
                .headers
                .iter()
                .map(|(k, v)| wit_types::KeyValuePair {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect(),
            body: self.body.clone(),
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
pub fn send(request: &Request) -> Result<Response, SysError> {
    let wit_req = request.to_wit();
    let result = wit_http::http_request(&wit_req).map_err(SysError::HostError)?;
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

/// Represents an active streaming HTTP response.
///
/// Must be explicitly closed via [`stream_close`] when done.
/// Not `Clone` — each handle is a unique owner of the host-side resource.
#[derive(Debug)]
pub struct HttpStreamHandle(pub(crate) u64);

impl HttpStreamHandle {
    /// Raw handle ID for interop with lower-level APIs.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// Metadata returned when a streaming HTTP request is initiated.
pub struct StreamStartResponse {
    /// The handle to use for subsequent [`stream_read`] / [`stream_close`] calls.
    pub handle: HttpStreamHandle,
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    pub headers: HashMap<String, String>,
}

/// Start a streaming HTTP request.
///
/// Sends the request and waits for the status/headers to arrive.
/// Returns a [`StreamStartResponse`] with the handle, status, and headers.
/// Use [`stream_read`] to consume the body in chunks.
pub fn stream_start(request: &Request) -> Result<StreamStartResponse, SysError> {
    let wit_req = request.to_wit();
    let result = wit_http::http_stream_start(&wit_req).map_err(SysError::HostError)?;
    let headers: HashMap<String, String> = result
        .headers
        .into_iter()
        .map(|kv| (kv.key, kv.value))
        .collect();
    Ok(StreamStartResponse {
        handle: HttpStreamHandle(result.handle),
        status: result.status,
        headers,
    })
}

/// Read the next chunk from a streaming HTTP response.
///
/// Returns `Ok(Some(bytes))` with the next chunk of data, or
/// `Ok(None)` when the stream is exhausted (EOF).
pub fn stream_read(stream: &HttpStreamHandle) -> Result<Option<Vec<u8>>, SysError> {
    let result = wit_http::http_stream_read(stream.0).map_err(SysError::HostError)?;
    if result.is_empty() {
        Ok(None)
    } else {
        Ok(Some(result))
    }
}

/// Close a streaming HTTP response, releasing host-side resources.
///
/// Idempotent — closing an already-closed handle is a no-op.
pub fn stream_close(stream: &HttpStreamHandle) -> Result<(), SysError> {
    wit_http::http_stream_close(stream.0).map_err(SysError::HostError)
}
