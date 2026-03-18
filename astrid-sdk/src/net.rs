use super::*;
use serde::{Deserialize, Serialize};

/// Represents a bound network listener.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerHandle(pub String);

/// Represents an open network stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamHandle(pub String);

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

/// Wire-format status byte prepended to every `astrid_net_read` response.
///
/// Matches `NetReadStatus` in the host `net.rs`. Internal — callers receive
/// [`TryRecvError`] or `Ok(bytes)` instead.
#[repr(u8)]
enum ReadStatus {
    Data = 0x00,
    Closed = 0x01,
    Pending = 0x02,
}

impl ReadStatus {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            b if b == Self::Data as u8 => Some(Self::Data),
            b if b == Self::Closed as u8 => Some(Self::Closed),
            b if b == Self::Pending as u8 => Some(Self::Pending),
            _ => None,
        }
    }
}

/// Bind a Unix Domain Socket to the given path and return a listener handle.
pub fn bind_unix(path: impl AsRef<[u8]>) -> Result<ListenerHandle, SysError> {
    let bytes = unsafe { astrid_net_bind_unix(path.as_ref().to_vec())? };
    let handle_str = String::from_utf8(bytes).map_err(|e| SysError::ApiError(e.to_string()))?;
    Ok(ListenerHandle(handle_str))
}

/// Block until the next incoming connection arrives on the listener.
pub fn accept(listener: &ListenerHandle) -> Result<StreamHandle, SysError> {
    let bytes = unsafe { astrid_net_accept(listener.0.as_bytes().to_vec())? };
    let handle_str = String::from_utf8(bytes).map_err(|e| SysError::ApiError(e.to_string()))?;
    Ok(StreamHandle(handle_str))
}

/// Non-blocking accept. Returns `Ok(Some(stream))` if a connection was
/// pending, `Ok(None)` if no connection is ready yet, or `Err` on a
/// listener error.
pub fn try_accept(listener: &ListenerHandle) -> Result<Option<StreamHandle>, SysError> {
    let bytes = unsafe { astrid_net_poll_accept(listener.0.as_bytes().to_vec())? };
    if bytes.is_empty() {
        return Ok(None);
    }
    let handle_str = String::from_utf8(bytes).map_err(|e| SysError::ApiError(e.to_string()))?;
    Ok(Some(StreamHandle(handle_str)))
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
                // try_recv blocks in the host for up to 50ms per call, so this
                // loop is not a true busy-wait. The sleep adds a small yield
                // between host calls for good measure.
                std::thread::sleep(std::time::Duration::from_millis(1));
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
    let bytes =
        unsafe { astrid_net_read(stream.0.as_bytes().to_vec()).map_err(|_| TryRecvError::Closed)? };
    // First byte is always the NetReadStatus discriminant (see host net.rs).
    let status = bytes
        .first()
        .and_then(|&b| ReadStatus::from_byte(b))
        .ok_or(TryRecvError::Closed)?;
    match status {
        ReadStatus::Data => Ok(bytes[1..].to_vec()),
        ReadStatus::Closed => Err(TryRecvError::Closed),
        ReadStatus::Pending => Err(TryRecvError::Empty),
    }
}

/// Send a message to the stream.
///
/// Returns `Err(SendError)` if the peer has disconnected and the message
/// could not be delivered.
///
/// Analogous to [`std::sync::mpsc::Sender::send`].
pub fn send(stream: &StreamHandle, data: &[u8]) -> Result<(), SendError> {
    unsafe {
        astrid_net_write(stream.0.as_bytes().to_vec(), data.to_vec()).map_err(|_| SendError)?
    };
    Ok(())
}

/// Close an open stream, releasing its resources on the host.
///
/// Idempotent — closing an already-closed handle is a no-op.
pub fn close(stream: &StreamHandle) -> Result<(), SysError> {
    unsafe { astrid_net_close_stream(stream.0.as_bytes().to_vec())? };
    Ok(())
}
