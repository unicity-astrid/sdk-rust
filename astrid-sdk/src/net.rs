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
