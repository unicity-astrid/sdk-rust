//! Networking — Unix-domain sockets, outbound TCP, UDP, and DNS lookup.
//!
//! Backed by `astrid:net/host@1.0.0`. Every kernel-side resource
//! (listener, stream, socket) is wrapped in a typed Rust handle whose
//! `Drop` releases the resource — no manual close on the happy path.
//!
//! The `astrid:net.tcp-stream` resource is shared between Unix-domain
//! accepted connections and outbound TCP; TCP-only socket options
//! (`set_nodelay`, `keepalive`, `set_hop_limit`, …) return
//! `host_err("NotTcp")` when called on a Unix-domain stream.

use super::*;

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

impl Shutdown {
    fn to_wit(self) -> wit_net::ShutdownHow {
        match self {
            Self::Read => wit_net::ShutdownHow::Receive,
            Self::Write => wit_net::ShutdownHow::Send,
            Self::Both => wit_net::ShutdownHow::Both,
        }
    }
}

/// Status of a framed read on a [`TcpStream`].
///
/// Mirrors `astrid:net/host.net-read-status`.
#[derive(Debug, Clone)]
pub enum ReadStatus {
    /// Data frame received.
    Data(Vec<u8>),
    /// Stream closed by peer.
    Closed,
    /// No data available right now (non-blocking).
    Pending,
}

impl ReadStatus {
    fn from_wit(status: wit_net::NetReadStatus) -> Self {
        match status {
            wit_net::NetReadStatus::Data(bytes) => Self::Data(bytes),
            wit_net::NetReadStatus::Closed => Self::Closed,
            wit_net::NetReadStatus::Pending => Self::Pending,
        }
    }
}

/// Error returned by [`TcpStream::recv`] when the stream is closed.
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

/// Error returned by [`TcpStream::try_recv`].
///
/// Mirrors [`std::sync::mpsc::TryRecvError`].
#[derive(Debug, PartialEq, Eq)]
pub enum TryRecvError {
    /// No message available yet — try again later.
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

/// Error returned by [`TcpStream::send`] when the stream is closed.
#[derive(Debug)]
pub struct SendError;

impl core::fmt::Display for SendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "stream closed")
    }
}

impl std::error::Error for SendError {}

/// The capsule's pre-bound Unix-domain listener.
///
/// Returned by [`bind_unix`]. RAII wraps the kernel-side resource;
/// drop releases the listener.
#[derive(Debug)]
pub struct UnixListener {
    inner: wit_net::UnixListener,
}

impl UnixListener {
    /// Blocking accept of the next inbound Unix-domain connection.
    pub fn accept(&self) -> Result<TcpStream, SysError> {
        let inner = self.inner.accept().map_err(host_err)?;
        Ok(TcpStream { inner })
    }

    /// Polling accept with a caller-controlled timeout (capped at
    /// 60_000 ms by the host). Returns `None` if no connection arrived
    /// within the deadline.
    pub fn try_accept(&self, timeout_ms: u64) -> Result<Option<TcpStream>, SysError> {
        let result = self.inner.poll_accept(timeout_ms).map_err(host_err)?;
        Ok(result.map(|inner| TcpStream { inner }))
    }
}

/// A bound TCP listener accepting inbound connections from the network.
///
/// Returned by [`bind_tcp`]. RAII over `astrid:net.tcp-listener`.
/// Per-capsule cap: 4 TCP listeners.
#[derive(Debug)]
pub struct TcpListener {
    inner: wit_net::TcpListener,
}

impl TcpListener {
    /// Blocking accept of the next inbound TCP connection.
    pub fn accept(&self) -> Result<TcpStream, SysError> {
        let inner = self.inner.accept().map_err(host_err)?;
        Ok(TcpStream { inner })
    }

    /// Polling accept with caller-controlled timeout.
    pub fn try_accept(&self, timeout_ms: u64) -> Result<Option<TcpStream>, SysError> {
        let result = self.inner.poll_accept(timeout_ms).map_err(host_err)?;
        Ok(result.map(|inner| TcpStream { inner }))
    }

    /// Local bind address as `"ip:port"`.
    pub fn local_addr(&self) -> Result<String, SysError> {
        self.inner.local_addr().map_err(host_err)
    }
}

/// A bidirectional byte stream — outbound TCP, inbound TCP-listener
/// accepts, and Unix-domain accepts all use this type.
///
/// Owns the kernel-side `astrid:net.tcp-stream` resource. Drop closes
/// the stream automatically. Per-capsule cap: 8 streams.
#[derive(Debug)]
pub struct TcpStream {
    inner: wit_net::TcpStream,
}

impl TcpStream {
    /// Open an outbound TCP connection to `host:port`.
    ///
    /// DNS resolution and the SSRF airlock run host-side; the WASM
    /// guest only sees the parsed host and port. The capsule manifest
    /// must allowlist `host:port` via `net_connect`.
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        let (host, port) = parse_host_port(addr)?;
        let inner = wit_net::connect_tcp(host, port).map_err(|e| {
            std::io::Error::other(format!("{e:?}"))
        })?;
        Ok(Self { inner })
    }

    // ---- Length-prefixed framed I/O (uplink-proxy use case) ----

    /// Read the next length-prefixed frame.
    pub fn read(&self) -> Result<ReadStatus, SysError> {
        self.inner.read().map(ReadStatus::from_wit).map_err(host_err)
    }

    /// Write a length-prefixed frame.
    pub fn write(&self, data: &[u8]) -> Result<(), SysError> {
        self.inner.write(data).map_err(host_err)
    }

    /// Receive the next frame, blocking until one arrives (sleep-polls
    /// against `read` at 50 ms intervals).
    ///
    /// Returns `Err(RecvError)` if the peer has disconnected.
    /// Analogous to [`std::sync::mpsc::Receiver::recv`].
    pub fn recv(&self) -> Result<Vec<u8>, RecvError> {
        loop {
            match self.try_recv() {
                Ok(bytes) => return Ok(bytes),
                Err(TryRecvError::Closed) => return Err(RecvError),
                Err(TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }

    /// Non-blocking frame receive.
    pub fn try_recv(&self) -> Result<Vec<u8>, TryRecvError> {
        match self.inner.read() {
            Ok(wit_net::NetReadStatus::Data(bytes)) => Ok(bytes),
            Ok(wit_net::NetReadStatus::Closed) => Err(TryRecvError::Closed),
            Ok(wit_net::NetReadStatus::Pending) => Err(TryRecvError::Empty),
            Err(_) => Err(TryRecvError::Closed),
        }
    }

    /// Send a framed message.
    pub fn send(&self, data: &[u8]) -> Result<(), SendError> {
        self.inner.write(data).map_err(|_| SendError)
    }

    // ---- Byte-stream I/O (general protocols) ----

    /// Read up to `max_bytes` from the stream without length-prefix
    /// framing. Mirrors `std::io::Read::read`.
    pub fn read_bytes(&self, max_bytes: u32) -> Result<Vec<u8>, SysError> {
        self.inner.read_bytes(max_bytes).map_err(host_err)
    }

    /// Write bytes without framing. Returns bytes actually written.
    pub fn write_bytes(&self, data: &[u8]) -> Result<u32, SysError> {
        self.inner.write_bytes(data).map_err(host_err)
    }

    /// Peek up to `max_bytes` without consuming them. Mirrors
    /// `std::net::TcpStream::peek`.
    pub fn peek(&self, max_bytes: u32) -> Result<Vec<u8>, SysError> {
        self.inner.peek(max_bytes).map_err(host_err)
    }

    // ---- Lifecycle ----

    /// Half-close the read side, write side, or both.
    pub fn shutdown(&self, how: Shutdown) -> Result<(), SysError> {
        self.inner.shutdown(how.to_wit()).map_err(host_err)
    }

    // ---- Address accessors ----

    /// Remote peer address as `"ip:port"`. Returns `host_err("NotTcp")`
    /// on Unix-domain streams.
    pub fn peer_addr(&self) -> Result<String, SysError> {
        self.inner.peer_addr().map_err(host_err)
    }

    /// Local socket address as `"ip:port"`. Returns `host_err("NotTcp")`
    /// on Unix-domain streams.
    pub fn local_addr(&self) -> Result<String, SysError> {
        self.inner.local_addr().map_err(host_err)
    }

    // ---- TCP socket options (`not_tcp` on Unix streams) ----

    /// Enable / disable `TCP_NODELAY` (Nagle off when `true`).
    pub fn set_nodelay(&self, nodelay: bool) -> Result<(), SysError> {
        self.inner.set_nodelay(nodelay).map_err(host_err)
    }

    /// Current `TCP_NODELAY` setting.
    pub fn nodelay(&self) -> Result<bool, SysError> {
        self.inner.nodelay().map_err(host_err)
    }

    /// Set the read timeout. `None` clears it; zero-duration is
    /// rejected (matches `std::net::TcpStream::set_read_timeout`).
    pub fn set_read_timeout(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<(), SysError> {
        let ms = to_host_timeout(timeout)?;
        self.inner.set_read_timeout(ms).map_err(host_err)
    }

    /// Current read timeout, or `None` if unset.
    pub fn read_timeout(&self) -> Result<Option<std::time::Duration>, SysError> {
        Ok(self
            .inner
            .read_timeout()
            .map_err(host_err)?
            .map(std::time::Duration::from_millis))
    }

    /// Set the write timeout. `None` clears it; zero-duration is rejected.
    pub fn set_write_timeout(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<(), SysError> {
        let ms = to_host_timeout(timeout)?;
        self.inner.set_write_timeout(ms).map_err(host_err)
    }

    /// Current write timeout, or `None` if unset.
    pub fn write_timeout(&self) -> Result<Option<std::time::Duration>, SysError> {
        Ok(self
            .inner
            .write_timeout()
            .map_err(host_err)?
            .map(std::time::Duration::from_millis))
    }

    /// Set the IPv6 hop limit / IPv4 TTL on outgoing packets.
    pub fn set_hop_limit(&self, hops: u32) -> Result<(), SysError> {
        self.inner.set_hop_limit(hops).map_err(host_err)
    }

    /// Current IPv6 hop limit / IPv4 TTL.
    pub fn hop_limit(&self) -> Result<u32, SysError> {
        self.inner.hop_limit().map_err(host_err)
    }

    /// Set the TCP keepalive interval. `None` disables; `Some(seconds)`
    /// enables with the given probe interval.
    pub fn set_keepalive(&self, keepalive_secs: Option<u64>) -> Result<(), SysError> {
        self.inner.set_keepalive(keepalive_secs).map_err(host_err)
    }

    /// Current keepalive setting.
    pub fn keepalive(&self) -> Result<Option<u64>, SysError> {
        self.inner.keepalive().map_err(host_err)
    }

    /// Set `SO_LINGER`. `None` = default graceful close;
    /// `Some(Duration::ZERO)` = immediate RST drop unsent; `Some(d)` =
    /// drain up to `d`.
    pub fn set_linger(&self, linger: Option<std::time::Duration>) -> Result<(), SysError> {
        let ms = linger.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        self.inner.set_linger(ms).map_err(host_err)
    }

    /// Current `SO_LINGER` setting.
    pub fn linger(&self) -> Result<Option<std::time::Duration>, SysError> {
        Ok(self
            .inner
            .linger()
            .map_err(host_err)?
            .map(std::time::Duration::from_millis))
    }

    /// Toggle `SO_REUSEADDR`. Has no effect on outbound streams.
    pub fn set_reuseaddr(&self, reuse: bool) -> Result<(), SysError> {
        self.inner.set_reuseaddr(reuse).map_err(host_err)
    }

    /// Current `SO_REUSEADDR` setting.
    pub fn reuseaddr(&self) -> Result<bool, SysError> {
        self.inner.reuseaddr().map_err(host_err)
    }

    // ---- Deprecated TTL aliases (pre-migration API) ----

    /// Backward-compat alias for [`set_hop_limit`].
    pub fn set_ttl(&self, ttl: u32) -> Result<(), SysError> {
        self.set_hop_limit(ttl)
    }

    /// Backward-compat alias for [`hop_limit`].
    pub fn ttl(&self) -> Result<u32, SysError> {
        self.hop_limit()
    }
}

impl std::io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let max = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let bytes = self.inner.read_bytes(max).map_err(io_error_from_net_op)?;
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

impl std::io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write_bytes(buf).map_err(io_error_from_net_op)?;
        Ok(n as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // The host writes are non-buffered at the SDK layer.
        Ok(())
    }
}

/// A UDP datagram socket.
///
/// Modes:
/// - **Unconnected** (default after [`udp_bind`]) — use [`UdpSocket::send_to`]
///   / [`UdpSocket::recv_from`] with per-call peer addressing.
/// - **Connected** (after [`UdpSocket::connect`]) — use [`UdpSocket::send`]
///   / [`UdpSocket::recv`] without per-call peer; faster syscall path
///   for chatty protocols.
#[derive(Debug)]
pub struct UdpSocket {
    inner: wit_net::UdpSocket,
}

/// A UDP datagram returned by [`UdpSocket::recv_from`].
#[derive(Debug, Clone)]
pub struct UdpDatagram {
    /// Payload bytes received.
    pub data: Vec<u8>,
    /// Peer host (numeric IP).
    pub peer_host: String,
    /// Peer UDP port.
    pub peer_port: u16,
}

impl UdpSocket {
    /// Send to a peer (unconnected mode). Returns bytes sent.
    pub fn send_to(&self, data: &[u8], peer_host: &str, peer_port: u16) -> Result<u32, SysError> {
        self.inner
            .send_to(data, peer_host, peer_port)
            .map_err(host_err)
    }

    /// Receive up to `max_bytes` (unconnected mode). `None` if no
    /// datagram arrived within the read timeout.
    pub fn recv_from(&self, max_bytes: u32) -> Result<Option<UdpDatagram>, SysError> {
        Ok(self
            .inner
            .recv_from(max_bytes)
            .map_err(host_err)?
            .map(|d| UdpDatagram {
                data: d.data,
                peer_host: d.peer_host,
                peer_port: d.peer_port,
            }))
    }

    /// Lock to a single peer (connected mode). The SSRF airlock runs
    /// once at connect time.
    pub fn connect(&self, peer_host: &str, peer_port: u16) -> Result<(), SysError> {
        self.inner.connect(peer_host, peer_port).map_err(host_err)
    }

    /// Unbind from the connected peer; revert to unconnected mode.
    pub fn disconnect(&self) -> Result<(), SysError> {
        self.inner.disconnect().map_err(host_err)
    }

    /// Send to the connected peer (connected mode). Returns bytes sent.
    pub fn send(&self, data: &[u8]) -> Result<u32, SysError> {
        self.inner.send(data).map_err(host_err)
    }

    /// Receive from the connected peer. `None` if no datagram arrived
    /// within the read timeout.
    pub fn recv(&self, max_bytes: u32) -> Result<Option<Vec<u8>>, SysError> {
        self.inner.recv(max_bytes).map_err(host_err)
    }

    /// Currently connected peer as `"ip:port"`. `None` if unconnected.
    pub fn peer_addr(&self) -> Result<Option<String>, SysError> {
        self.inner.peer_addr().map_err(host_err)
    }

    /// Set the read timeout for `recv_from` / `recv`. `None` clears.
    pub fn set_read_timeout(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<(), SysError> {
        let ms = to_host_timeout(timeout)?;
        self.inner.set_read_timeout(ms).map_err(host_err)
    }

    /// Local bound address as `"ip:port"`.
    pub fn local_addr(&self) -> Result<String, SysError> {
        self.inner.local_addr().map_err(host_err)
    }
}

// ---------------------------------------------------------------------------
// Factory functions
// ---------------------------------------------------------------------------

/// Bind and activate the capsule's pre-provisioned Unix-domain listener.
///
/// The kernel pre-binds the socket at capsule load time; this call
/// activates it. Returns a [`UnixListener`] whose `Drop` releases the
/// resource.
pub fn bind_unix() -> Result<UnixListener, SysError> {
    let inner = wit_net::bind_unix().map_err(host_err)?;
    Ok(UnixListener { inner })
}

/// Bind a TCP listener for inbound connections.
///
/// Restricted by the `net_tcp_bind` capability allowlist. Port `0`
/// selects an ephemeral port.
pub fn bind_tcp(host: &str, port: u16) -> Result<TcpListener, SysError> {
    let inner = wit_net::bind_tcp(host, port).map_err(host_err)?;
    Ok(TcpListener { inner })
}

/// Open an outbound TCP connection. Requires `net_connect` allowlist
/// match.
pub fn connect(host: &str, port: u16) -> Result<TcpStream, SysError> {
    let inner = wit_net::connect_tcp(host, port).map_err(host_err)?;
    Ok(TcpStream { inner })
}

/// Bind a UDP socket. Requires `net_udp` allowlist match.
pub fn udp_bind(host: &str, port: u16) -> Result<UdpSocket, SysError> {
    let inner = wit_net::udp_bind(host, port).map_err(host_err)?;
    Ok(UdpSocket { inner })
}

/// Resolve a hostname to a list of `"ip:port"` (or `"ip"`) strings.
/// SSRF airlock applies — private/loopback/etc. ranges are stripped.
pub fn lookup_host(host: &str) -> Result<Vec<String>, SysError> {
    wit_net::lookup_host(host).map_err(host_err)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// `io::Error` from a network-op `SysError`. Maps `WouldBlock`-flavoured
/// host errors to [`std::io::ErrorKind::WouldBlock`] so std-style
/// callers can distinguish "timeout fired, retry" from a real EOF or
/// transport error.
fn io_error_from_net_op<E: core::fmt::Debug>(err: E) -> std::io::Error {
    let msg = format!("{err:?}");
    if msg.contains("WouldBlock") || msg.contains("would block") {
        std::io::Error::new(std::io::ErrorKind::WouldBlock, msg)
    } else {
        std::io::Error::other(msg)
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
            to_host_timeout(Some(std::time::Duration::from_millis(500))).unwrap(),
            Some(500)
        );
    }
}
