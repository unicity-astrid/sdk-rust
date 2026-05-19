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

// ---------------------------------------------------------------------------
// Outbound TCP — std::net::TcpStream parity
// ---------------------------------------------------------------------------

/// Direction argument for [`TcpStream::shutdown`] — mirror of
/// [`std::net::Shutdown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    /// Half-close the read side.
    Read,
    /// Half-close the write side.
    Write,
    /// Close both directions.
    Both,
}

/// Open an outbound TCP connection to `host:port` and return a stream handle.
///
/// The capsule's `Capsule.toml` must declare a `net_connect` allowlist entry
/// matching `host:port` (exact `"host:port"` or `"host:*"`); missing or empty
/// allowlist denies all outbound TCP. The kernel runs the same SSRF airlock
/// used by `http-request` on the resolved IP and enforces a connect timeout
/// (10s default).
///
/// The returned handle flows through the byte-stream surface
/// ([`read_bytes`] / [`write_bytes`] / [`close`]) and the std-shaped
/// [`TcpStream`] facade. The frame-oriented [`recv`] / [`send`] also work
/// but are intended for the inbound Unix-accept proxy use case.
pub fn connect(host: &str, port: u16) -> Result<StreamHandle, SysError> {
    let handle = wit_net::net_connect_tcp(host, port).map_err(SysError::HostError)?;
    Ok(StreamHandle(handle))
}

/// Read up to `max_bytes` from `stream` without length-prefix framing.
///
/// Mirrors `std::io::Read::read`. Empty result means EOF (peer
/// disconnected). Honours any read timeout previously set via
/// [`set_read_timeout`].
pub fn read_bytes(stream: &StreamHandle, max_bytes: u32) -> Result<Vec<u8>, SysError> {
    wit_net::net_read_bytes(stream.0, max_bytes).map_err(SysError::HostError)
}

/// Write `data` to `stream` without framing. Returns the number of bytes
/// actually written (may be less than `data.len()`). Honours any write
/// timeout previously set via [`set_write_timeout`].
pub fn write_bytes(stream: &StreamHandle, data: &[u8]) -> Result<u32, SysError> {
    wit_net::net_write_bytes(stream.0, data).map_err(SysError::HostError)
}

/// Peek up to `max_bytes` without consuming them — the next
/// [`read_bytes`] returns the same data again.
pub fn peek(stream: &StreamHandle, max_bytes: u32) -> Result<Vec<u8>, SysError> {
    wit_net::net_peek(stream.0, max_bytes).map_err(SysError::HostError)
}

/// Half-close the read side, write side, or both.
pub fn shutdown(stream: &StreamHandle, how: Shutdown) -> Result<(), SysError> {
    let wit_how = match how {
        Shutdown::Read => wit_types::ShutdownHow::Read,
        Shutdown::Write => wit_types::ShutdownHow::Write,
        Shutdown::Both => wit_types::ShutdownHow::Both,
    };
    wit_net::net_shutdown(stream.0, wit_how).map_err(SysError::HostError)
}

/// Remote peer address, formatted as `"ip:port"`.
pub fn peer_addr(stream: &StreamHandle) -> Result<String, SysError> {
    wit_net::net_peer_addr(stream.0).map_err(SysError::HostError)
}

/// Local socket address, formatted as `"ip:port"`.
pub fn local_addr(stream: &StreamHandle) -> Result<String, SysError> {
    wit_net::net_local_addr(stream.0).map_err(SysError::HostError)
}

/// Toggle `TCP_NODELAY` (Nagle off when `true`).
pub fn set_nodelay(stream: &StreamHandle, nodelay: bool) -> Result<(), SysError> {
    wit_net::net_set_nodelay(stream.0, nodelay).map_err(SysError::HostError)
}

/// Current `TCP_NODELAY` setting.
pub fn nodelay(stream: &StreamHandle) -> Result<bool, SysError> {
    wit_net::net_nodelay(stream.0).map_err(SysError::HostError)
}

/// Validate + convert a timeout to milliseconds for the host fn.
///
/// Mirrors `std::net::TcpStream::set_read_timeout` /
/// `set_write_timeout`, which both reject `Some(Duration::ZERO)` —
/// zero would be ambiguous with "no timeout".
fn to_host_timeout(timeout: Option<std::time::Duration>) -> Result<Option<u64>, SysError> {
    match timeout {
        Some(d) if d.is_zero() => Err(SysError::HostError(
            "timeout must be non-zero (use None to clear)".into(),
        )),
        Some(d) => Ok(Some(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))),
        None => Ok(None),
    }
}

/// Set the read timeout. `None` clears it; `Some(Duration::ZERO)` is
/// rejected (matches `std::net::TcpStream::set_read_timeout`).
pub fn set_read_timeout(
    stream: &StreamHandle,
    timeout: Option<std::time::Duration>,
) -> Result<(), SysError> {
    let ms = to_host_timeout(timeout)?;
    wit_net::net_set_read_timeout(stream.0, ms).map_err(SysError::HostError)
}

/// Current read timeout, or `None` if unset.
pub fn read_timeout(stream: &StreamHandle) -> Result<Option<std::time::Duration>, SysError> {
    Ok(wit_net::net_read_timeout(stream.0)
        .map_err(SysError::HostError)?
        .map(std::time::Duration::from_millis))
}

/// Set the write timeout. `None` clears it; `Some(Duration::ZERO)` is
/// rejected (matches `std::net::TcpStream::set_write_timeout`).
pub fn set_write_timeout(
    stream: &StreamHandle,
    timeout: Option<std::time::Duration>,
) -> Result<(), SysError> {
    let ms = to_host_timeout(timeout)?;
    wit_net::net_set_write_timeout(stream.0, ms).map_err(SysError::HostError)
}

/// Current write timeout, or `None` if unset.
pub fn write_timeout(stream: &StreamHandle) -> Result<Option<std::time::Duration>, SysError> {
    Ok(wit_net::net_write_timeout(stream.0)
        .map_err(SysError::HostError)?
        .map(std::time::Duration::from_millis))
}

/// Set the IP TTL on outgoing packets.
pub fn set_ttl(stream: &StreamHandle, ttl: u32) -> Result<(), SysError> {
    wit_net::net_set_ttl(stream.0, ttl).map_err(SysError::HostError)
}

/// Current IP TTL.
pub fn ttl(stream: &StreamHandle) -> Result<u32, SysError> {
    wit_net::net_ttl(stream.0).map_err(SysError::HostError)
}

/// A connected TCP stream — the SDK analogue of [`std::net::TcpStream`].
///
/// Owns a [`StreamHandle`] from [`connect`] and implements
/// [`std::io::Read`] and [`std::io::Write`] over the host's byte-stream
/// `net-read-bytes` / `net-write-bytes` (no length-prefix framing). Generic
/// code that operates on any `Read + Write` (TLS clients, WebSocket
/// libraries, Postgres drivers) works unmodified.
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
/// sock.set_nodelay(true)?;
/// sock.write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n")?;
///
/// let mut buf = vec![0u8; 4096];
/// let n = sock.read(&mut buf)?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct TcpStream {
    handle: StreamHandle,
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
        Ok(Self { handle })
    }

    /// Wrap an existing [`StreamHandle`] in the `TcpStream` facade. The
    /// `TcpStream` takes ownership and will close the handle on drop.
    #[must_use]
    pub fn from_handle(handle: StreamHandle) -> Self {
        Self { handle }
    }

    /// The raw stream handle. Use the free functions in this module if you
    /// need the byte-stream surface directly.
    #[must_use]
    pub fn handle(&self) -> &StreamHandle {
        &self.handle
    }

    /// Set the `TCP_NODELAY` socket option (Nagle's algorithm off when `true`).
    pub fn set_nodelay(&self, nodelay: bool) -> std::io::Result<()> {
        set_nodelay(&self.handle, nodelay).map_err(io_error_from_sys)
    }

    /// Read the current `TCP_NODELAY` setting.
    pub fn nodelay(&self) -> std::io::Result<bool> {
        nodelay(&self.handle).map_err(io_error_from_sys)
    }

    /// Set the read timeout. `None` clears it.
    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        set_read_timeout(&self.handle, timeout).map_err(io_error_from_sys)
    }

    /// Current read timeout.
    pub fn read_timeout(&self) -> std::io::Result<Option<std::time::Duration>> {
        read_timeout(&self.handle).map_err(io_error_from_sys)
    }

    /// Set the write timeout. `None` clears it.
    pub fn set_write_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        set_write_timeout(&self.handle, timeout).map_err(io_error_from_sys)
    }

    /// Current write timeout.
    pub fn write_timeout(&self) -> std::io::Result<Option<std::time::Duration>> {
        write_timeout(&self.handle).map_err(io_error_from_sys)
    }

    /// Set the IP `TTL` on outgoing packets.
    pub fn set_ttl(&self, ttl_val: u32) -> std::io::Result<()> {
        set_ttl(&self.handle, ttl_val).map_err(io_error_from_sys)
    }

    /// Current IP `TTL`.
    pub fn ttl(&self) -> std::io::Result<u32> {
        ttl(&self.handle).map_err(io_error_from_sys)
    }

    /// Remote peer address as `"ip:port"`.
    pub fn peer_addr(&self) -> std::io::Result<String> {
        peer_addr(&self.handle).map_err(io_error_from_sys)
    }

    /// Local socket address as `"ip:port"`.
    pub fn local_addr(&self) -> std::io::Result<String> {
        local_addr(&self.handle).map_err(io_error_from_sys)
    }

    /// Half-close the read side, write side, or both.
    pub fn shutdown(&self, how: Shutdown) -> std::io::Result<()> {
        shutdown(&self.handle, how).map_err(io_error_from_sys)
    }

    /// Peek up to `buf.len()` bytes without consuming them. Returns the
    /// number of bytes written into `buf`. `Ok(0)` is EOF (matches
    /// `std::net::TcpStream::peek`). If a read timeout is set and it
    /// expires with no data, returns
    /// [`std::io::ErrorKind::WouldBlock`].
    pub fn peek(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        let max = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let bytes = peek(&self.handle, max).map_err(io_error_from_net_op)?;
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

impl std::io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let max = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let bytes = read_bytes(&self.handle, max).map_err(io_error_from_net_op)?;
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

impl std::io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = write_bytes(&self.handle, buf).map_err(io_error_from_net_op)?;
        Ok(n as usize)
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

/// Parse `"host:port"` for [`TcpStream::connect`]. Strips IPv6 square
/// brackets — `[::1]:443` → `("::1", 443)` — because the host fn takes
/// a raw hostname/IP without brackets (matching `tokio::net::lookup_host`
/// and `std`'s `(&str, u16)` `ToSocketAddrs` impl).
fn parse_host_port(addr: &str) -> std::io::Result<(&str, u16)> {
    // IPv6 literal in `[v6]:port` form — split on the closing bracket
    // so a v6 address with internal colons doesn't fool `rsplit_once`.
    if let Some(end) = addr.strip_prefix('[') {
        let (v6, rest) = end.split_once(']').ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing closing `]` in IPv6 address",
            )
        })?;
        let port_str = rest.strip_prefix(':').ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "IPv6 address must be followed by `:port`",
            )
        })?;
        let port = parse_port(port_str)?;
        return Ok((v6, port));
    }
    let (host, port_str) = addr.rsplit_once(':').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "address must be \"host:port\"",
        )
    })?;
    Ok((host, parse_port(port_str)?))
}

fn parse_port(port_str: &str) -> std::io::Result<u16> {
    port_str.parse::<u16>().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid port: {e}"),
        )
    })
}

fn io_error_from_sys(err: SysError) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

/// `io::Error` from a network-op `SysError`, mapping the host's
/// `"... would block"` sentinel to [`std::io::ErrorKind::WouldBlock`]
/// so std-style callers (`Read::read`, `Write::write`, `peek`) can
/// distinguish "timeout fired, retry" from a real EOF or transport
/// error.
fn io_error_from_net_op(err: SysError) -> std::io::Error {
    let msg = err.to_string();
    if msg.contains("would block") {
        std::io::Error::new(std::io::ErrorKind::WouldBlock, msg)
    } else {
        std::io::Error::other(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_host_port_basic() {
        let (h, p) = parse_host_port("example.com:443").unwrap();
        assert_eq!((h, p), ("example.com", 443));
    }

    #[test]
    fn parse_host_port_strips_ipv6_brackets() {
        let (h, p) = parse_host_port("[::1]:443").unwrap();
        assert_eq!((h, p), ("::1", 443));
        let (h, p) = parse_host_port("[2001:db8::1]:8080").unwrap();
        assert_eq!((h, p), ("2001:db8::1", 8080));
    }

    #[test]
    fn parse_host_port_ipv6_without_brackets_fails_cleanly() {
        // `2001:db8::1:443` is ambiguous (no brackets, multiple colons).
        // rsplit_once takes the last colon — the port parse then fails
        // because `:1` precedes it. We accept this; brackets are the
        // unambiguous form.
        assert!(parse_host_port("2001:db8::1:abc").is_err());
    }

    #[test]
    fn parse_host_port_missing_close_bracket() {
        assert!(parse_host_port("[::1:443").is_err());
    }

    #[test]
    fn parse_host_port_invalid_port_rejected() {
        assert!(parse_host_port("example.com:notaport").is_err());
        assert!(parse_host_port("example.com:99999").is_err()); // exceeds u16
    }

    #[test]
    fn to_host_timeout_rejects_zero() {
        let err = to_host_timeout(Some(std::time::Duration::ZERO)).unwrap_err();
        assert!(err.to_string().contains("non-zero"));
    }

    #[test]
    fn to_host_timeout_passes_through() {
        assert_eq!(to_host_timeout(None).unwrap(), None);
        assert_eq!(
            to_host_timeout(Some(std::time::Duration::from_millis(500)))
                .unwrap(),
            Some(500)
        );
    }

    #[test]
    fn io_error_from_net_op_maps_would_block() {
        let err = io_error_from_net_op(SysError::HostError("read would block".into()));
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn io_error_from_net_op_other_passes_through() {
        let err = io_error_from_net_op(SysError::HostError("dns: no addresses".into()));
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }
}
