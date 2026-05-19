use super::*;

/// Represents a bound network listener.
#[derive(Debug, Clone)]
pub struct ListenerHandle(pub(crate) u64);

impl ListenerHandle {
    /// Raw handle ID for interop with lower-level APIs.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// Represents an open network stream.
#[derive(Debug, Clone)]
pub struct StreamHandle(pub(crate) u64);

impl StreamHandle {
    /// Raw handle ID for interop with lower-level APIs.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// Error returned by [`recv`] when the stream is closed.
///
/// Mirrors [`std::sync::mpsc::RecvError`] — the only reason a blocking
/// receive fails is that the peer has disconnected.
#[derive(Debug)]
pub struct RecvError;

impl core::fmt::Display for RecvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "stream closed")
    }
}

impl std::error::Error for RecvError {}

/// Error returned by [`try_recv`] when no message is ready or the stream
/// is closed.
///
/// Mirrors [`std::sync::mpsc::TryRecvError`].
#[derive(Debug, PartialEq, Eq)]
pub enum TryRecvError {
    /// No message is available yet — try again later.
    Empty,
    /// The peer has disconnected and no more messages will arrive.
    Closed,
}

impl core::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "no message available"),
            Self::Closed => write!(f, "stream closed"),
        }
    }
}

impl std::error::Error for TryRecvError {}

/// Error returned by [`send`] when the stream is closed and the message
/// could not be delivered.
///
/// Mirrors [`std::sync::mpsc::SendError`].
#[derive(Debug)]
pub struct SendError;

impl core::fmt::Display for SendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "stream closed")
    }
}

impl std::error::Error for SendError {}

/// Bind the kernel-provisioned Unix Domain Socket and return a listener handle.
///
/// The kernel pre-provisions a single Unix socket per capsule. Use
/// [`crate::runtime::socket_path()`] to discover the actual socket path.
pub fn bind_unix() -> Result<ListenerHandle, SysError> {
    let handle = wit_net::net_bind_unix(0).map_err(SysError::HostError)?;
    Ok(ListenerHandle(handle))
}

/// Block until the next incoming connection arrives on the listener.
pub fn accept(listener: &ListenerHandle) -> Result<StreamHandle, SysError> {
    let handle = wit_net::net_accept(listener.0).map_err(SysError::HostError)?;
    Ok(StreamHandle(handle))
}

/// Non-blocking accept. Returns `Ok(Some(stream))` if a connection was
/// pending, `Ok(None)` if no connection is ready yet, or `Err` on a
/// listener error.
pub fn try_accept(listener: &ListenerHandle) -> Result<Option<StreamHandle>, SysError> {
    let result = wit_net::net_poll_accept(listener.0).map_err(SysError::HostError)?;
    Ok(result.map(StreamHandle))
}

/// Receive the next message from the stream, blocking until one arrives.
///
/// Returns `Err(RecvError)` if the peer has disconnected.
///
/// Analogous to [`std::sync::mpsc::Receiver::recv`].
pub fn recv(stream: &StreamHandle) -> Result<Vec<u8>, RecvError> {
    loop {
        match try_recv(stream) {
            Ok(bytes) => return Ok(bytes),
            Err(TryRecvError::Closed) => return Err(RecvError),
            Err(TryRecvError::Empty) => {
                // The WIT net-read function is non-blocking. This is a polling
                // loop — sleep between attempts to avoid spinning the CPU.
                // 50ms balances responsiveness with CPU usage.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// Receive the next message from the stream without blocking.
///
/// Returns:
/// - `Ok(bytes)` — a message is available
/// - `Err(TryRecvError::Empty)` — no message ready yet, try again later
/// - `Err(TryRecvError::Closed)` — peer has disconnected
///
/// Analogous to [`std::sync::mpsc::Receiver::try_recv`].
pub fn try_recv(stream: &StreamHandle) -> Result<Vec<u8>, TryRecvError> {
    let status = wit_net::net_read(stream.0).map_err(|_| TryRecvError::Closed)?;
    match status {
        wit_types::NetReadStatus::Data(bytes) => Ok(bytes),
        wit_types::NetReadStatus::Closed => Err(TryRecvError::Closed),
        wit_types::NetReadStatus::Pending => Err(TryRecvError::Empty),
    }
}

/// Send a message to the stream.
///
/// Returns `Err(SendError)` if the peer has disconnected and the message
/// could not be delivered.
///
/// Analogous to [`std::sync::mpsc::Sender::send`].
pub fn send(stream: &StreamHandle, data: &[u8]) -> Result<(), SendError> {
    wit_net::net_write(stream.0, data).map_err(|_| SendError)
}

/// Close an open stream, releasing its resources on the host.
///
/// Idempotent — closing an already-closed handle is a no-op.
pub fn close(stream: &StreamHandle) -> Result<(), SysError> {
    wit_net::net_close_stream(stream.0).map_err(SysError::HostError)
}

/// Open an outbound TCP connection to `host:port` and return a stream handle.
///
/// The capsule's `Capsule.toml` must declare a `net_connect` allowlist entry
/// matching `host:port` (exact `"host:port"` or `"host:*"`); missing or empty
/// allowlist denies all outbound TCP. The kernel runs the same SSRF airlock
/// used by `http-request` on the resolved IP and enforces a connect timeout
/// (10s default).
///
/// The returned handle flows through the same [`send`] / [`recv`] / [`try_recv`]
/// / [`close`] surface as a handle from [`accept`]. For a `std::net::TcpStream`-
/// shaped facade with [`std::io::Read`] / [`std::io::Write`], see [`TcpStream`].
pub fn connect(host: &str, port: u16) -> Result<StreamHandle, SysError> {
    let handle = wit_net::net_connect_tcp(host, port).map_err(SysError::HostError)?;
    Ok(StreamHandle(handle))
}

/// A connected TCP stream — the SDK analogue of [`std::net::TcpStream`].
///
/// Owns a [`StreamHandle`] from [`connect`] (or [`accept`]) and implements
/// [`std::io::Read`] and [`std::io::Write`] so generic code that operates on
/// any `Read + Write` (TLS clients, WebSocket libraries, Postgres drivers)
/// works unmodified.
///
/// The host closes the underlying stream when this value is dropped, so
/// `TcpStream` is RAII — no explicit close required.
///
/// # Example
///
/// ```no_run
/// use astrid_sdk::net::TcpStream;
/// use std::io::{Read, Write};
///
/// let mut sock = TcpStream::connect("fulcrum.unicity.network:443")?;
/// sock.write_all(b"hello")?;
///
/// let mut buf = vec![0u8; 4096];
/// let n = sock.read(&mut buf)?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct TcpStream {
    handle: StreamHandle,
    /// Per-stream read buffer for the byte-stream [`std::io::Read`] facade.
    /// The host-side frame may be larger than the caller's `read` buffer;
    /// the surplus stays here until the next call consumes it.
    read_residual: Vec<u8>,
}

impl TcpStream {
    /// Open a TCP connection to `addr`, formatted as `"host:port"`.
    ///
    /// DNS resolution and the SSRF airlock run host-side; the WASM guest
    /// only sees the parsed host and port.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] wrapping the host-side failure
    /// (capability denial, SSRF rejection, DNS failure, connect timeout,
    /// or remote refusal).
    pub fn connect<A: AsRef<str>>(addr: A) -> std::io::Result<Self> {
        let (host, port) = parse_host_port(addr.as_ref())?;
        let handle = connect(host, port).map_err(io_error_from_sys)?;
        Ok(Self { handle, read_residual: Vec::new() })
    }

    /// Wrap an existing [`StreamHandle`] (e.g. one returned by [`accept`])
    /// in the `TcpStream` facade. The `TcpStream` takes ownership and will
    /// close the handle on drop.
    #[must_use]
    pub fn from_handle(handle: StreamHandle) -> Self {
        Self { handle, read_residual: Vec::new() }
    }

    /// The raw stream handle. Use [`send`] / [`recv`] directly if you need
    /// the frame-oriented API instead of `Read + Write`.
    #[must_use]
    pub fn handle(&self) -> &StreamHandle {
        &self.handle
    }
}

impl std::io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.read_residual.is_empty() {
            let n = buf.len().min(self.read_residual.len());
            buf[..n].copy_from_slice(&self.read_residual[..n]);
            self.read_residual.drain(..n);
            return Ok(n);
        }
        match recv(&self.handle) {
            Ok(frame) => {
                let n = buf.len().min(frame.len());
                buf[..n].copy_from_slice(&frame[..n]);
                if n < frame.len() {
                    self.read_residual = frame[n..].to_vec();
                }
                Ok(n)
            }
            // Peer disconnect is EOF in the std Read contract.
            Err(RecvError) => Ok(0),
        }
    }
}

impl std::io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        send(&self.handle, buf).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stream closed")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // The host writes are non-buffered at the SDK layer.
        Ok(())
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let _ = close(&self.handle);
    }
}

fn parse_host_port(addr: &str) -> std::io::Result<(&str, u16)> {
    let (host, port_str) = addr.rsplit_once(':').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "address must be \"host:port\"",
        )
    })?;
    let port = port_str.parse::<u16>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid port: {e}"),
        )
    })?;
    Ok((host, port))
}

fn io_error_from_sys(err: SysError) -> std::io::Error {
    std::io::Error::other(err.to_string())
}
