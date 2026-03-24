//! Canonical decoder for the `HostResult` wire format.
//!
//! Every host function that returns data from the kernel to a WASM guest
//! capsule encodes its response as:
//!
//! ```text
//! 0x00 + payload bytes  → Ok(payload)
//! 0x01 + UTF-8 message  → Err(message)
//! ```
//!
//! This module provides [`decode`] and [`decode_void`] — the single point
//! of decoding used by `fs`, `kv`, `http`, and all other SDK wrappers.
//!
//! # Legacy compatibility
//!
//! If the first byte is neither `0x00` nor `0x01`, the response is treated
//! as raw content (no prefix). This handles the case where a newer SDK runs
//! against an older kernel that hasn't adopted the `HostResult` convention
//! for a particular function yet.

use crate::SysError;

/// Status byte: successful result.
const STATUS_OK: u8 = 0x00;

/// Status byte: recoverable error.
const STATUS_ERR: u8 = 0x01;

/// Decode a `HostResult`-encoded response into `Result<Vec<u8>, SysError>`.
///
/// - `0x00` + payload → `Ok(payload)`
/// - `0x01` + message → `Err(SysError::ApiError(message))`
/// - Empty → `Ok(vec![])`
/// - Other first byte → `Ok(raw)` (legacy kernel compatibility)
pub(crate) fn decode(bytes: Vec<u8>) -> Result<Vec<u8>, SysError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    match bytes[0] {
        STATUS_OK => Ok(bytes[1..].to_vec()),
        STATUS_ERR => {
            let msg = String::from_utf8_lossy(&bytes[1..]).to_string();
            Err(SysError::ApiError(msg))
        }
        // Legacy: no prefix byte — treat entire response as raw content.
        _ => Ok(bytes),
    }
}

// decode_void will be added when void-returning host functions adopt HostResult.
