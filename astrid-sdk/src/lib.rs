//! Safe Rust SDK for building User-Space Capsules on Astrid OS.
//!
//! # Design Intent
//!
//! This SDK is meant to feel like using `std`. Module names, function
//! signatures, and type patterns follow Rust standard library conventions so
//! that a Rust developer's instinct for "where would I find X?" gives the
//! right answer without reading docs. When Astrid adds a concept that has no
//! `std` counterpart (IPC, capabilities, interceptors), the API still follows
//! the same style: typed handles, `Result`-based errors, and `impl AsRef`
//! parameters.
//!
//! See `docs/sdk-ergonomics.md` for the full design rationale.
//!
//! # Module Layout (mirrors `std` where applicable)
//!
//! | Module          | std equivalent   | Purpose                                |
//! |-----------------|------------------|----------------------------------------|
//! | [`fs`]          | `std::fs`        | Virtual filesystem                     |
//! | [`net`]         | `std::net`       | Unix domain sockets                    |
//! | [`process`]     | `std::process`   | Host process execution                 |
//! | [`env`]         | `std::env`       | Capsule configuration / env vars       |
//! | [`time`]        | `std::time`      | Wall-clock access                      |
//! | [`log`]         | `log` crate      | Structured logging                     |
//! | [`runtime`]     | N/A              | OS signaling and caller context        |
//! | [`ipc`]         | N/A              | Event bus messaging                    |
//! | [`kv`]          | N/A              | Persistent key-value storage           |
//! | [`http`]        | N/A              | Outbound HTTP requests                 |
//! | [`uplink`]      | N/A              | Direct frontend messaging              |
//! | [`hooks`]       | N/A              | User middleware triggers               |
//! | [`elicit`]      | N/A              | Interactive install/upgrade prompts    |
//! | [`identity`]    | N/A              | Platform user identity resolution      |
//! | [`approval`]    | N/A              | Human approval for sensitive actions   |
//! | [`types`]       | N/A              | IPC payload types and LLM schemas      |

#![allow(unsafe_code)]
#![allow(missing_docs)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use astrid_sys::astrid::capsule::{
    approval as wit_approval, elicit as wit_elicit, fs as wit_fs, http as wit_http,
    identity as wit_identity, ipc as wit_ipc, kv as wit_kv, net as wit_net, process as wit_process,
    sys as wit_sys, types as wit_types, uplink as wit_uplink,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

/// Shared Astrid data types (IPC payloads, LLM schemas, kernel API).
///
/// Re-exported from [`astrid_types`]. SDK-specific types like [`CallerContext`]
/// are also available here.
pub mod types {
    use serde::{Deserialize, Serialize};

    // Sub-modules (re-exported for `astrid_sdk::types::ipc::*` access)
    pub use astrid_types::ipc;
    pub use astrid_types::kernel;
    pub use astrid_types::llm;

    // IPC types
    pub use astrid_types::ipc::{
        IpcMessage, IpcPayload, OnboardingField, OnboardingFieldType, SelectionOption,
    };

    // Kernel API types
    pub use astrid_types::kernel::{
        CapsuleMetadataEntry, CommandInfo, KernelRequest, KernelResponse, LlmProviderInfo,
        SYSTEM_SESSION_UUID,
    };

    // LLM types
    pub use astrid_types::llm::{
        ContentPart, LlmResponse, LlmToolDefinition, Message, MessageContent, MessageRole,
        StopReason, StreamEvent, ToolCall, ToolCallResult, Usage,
    };

    /// Identifies the user and session that triggered the current capsule execution.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CallerContext {
        /// Session ID, if available.
        pub session_id: Option<String>,
        /// User ID, if available.
        pub user_id: Option<String>,
    }
}
pub use borsh;
pub use serde;
pub use serde_json;

// Re-exported for the #[capsule] macro's generated code. Not part of the
// public API - capsule authors should never need to import these directly.
#[doc(hidden)]
pub use astrid_sys;
#[doc(hidden)]
pub use schemars;

/// Core error type for SDK operations
#[derive(Error, Debug)]
pub enum SysError {
    #[error("Host function call failed: {0}")]
    HostError(String),
    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("MessagePack serialization error: {0}")]
    MsgPackEncodeError(#[from] rmp_serde::encode::Error),
    #[error("MessagePack deserialization error: {0}")]
    MsgPackDecodeError(#[from] rmp_serde::decode::Error),
    #[error("Borsh serialization error: {0}")]
    BorshError(#[from] std::io::Error),
    #[error("API logic error: {0}")]
    ApiError(String),
}

/// Convert bytes to a UTF-8 string, returning an error instead of silently
/// corrupting non-UTF-8 data. The Component Model WIT `string` type requires
/// valid UTF-8 — lossy conversion would silently garble binary payloads.
fn bytes_to_str(bytes: &[u8]) -> Result<&str, SysError> {
    std::str::from_utf8(bytes).map_err(|e| SysError::ApiError(format!("invalid UTF-8: {e}")))
}

pub mod fs;

/// Event bus messaging (like `std::sync::mpsc` but topic-based).
pub mod ipc {
    use super::*;

    /// An active subscription to an IPC topic. Returned by [`subscribe`].
    ///
    /// Follows the typed-handle pattern used by [`crate::net::ListenerHandle`].
    #[derive(Debug, Clone)]
    pub struct SubscriptionHandle(pub(crate) u64);

    impl SubscriptionHandle {
        /// Raw handle ID for interop with lower-level APIs.
        #[must_use]
        pub fn id(&self) -> u64 {
            self.0
        }

        /// Raw handle bytes for interop with lower-level APIs.
        #[must_use]
        pub fn as_bytes(&self) -> Vec<u8> {
            self.0.to_string().into_bytes()
        }
    }

    // Note: AsRef<[u8]> was removed — the handle now wraps u64, not bytes.
    // Use `.id()` to get the u64 handle or `.as_bytes()` for a Vec<u8>.

    pub fn publish_bytes(topic: impl AsRef<[u8]>, payload: &[u8]) -> Result<(), SysError> {
        let topic_str = bytes_to_str(topic.as_ref())?;
        let payload_str = bytes_to_str(payload)?;
        wit_ipc::ipc_publish(topic_str, payload_str).map_err(SysError::HostError)
    }

    pub fn publish_json<T: Serialize>(
        topic: impl AsRef<[u8]>,
        payload: &T,
    ) -> Result<(), SysError> {
        let bytes = serde_json::to_vec(payload)?;
        publish_bytes(topic, &bytes)
    }

    /// Publish a MessagePack-encoded value to an IPC topic.
    ///
    /// **Removed:** The IPC bus now carries UTF-8 strings (WIT `string`
    /// type). MessagePack is binary and cannot be sent as a string payload.
    /// Use [`publish_json`] instead.
    #[deprecated(note = "IPC bus carries UTF-8 strings. Use publish_json instead.")]
    pub fn publish_msgpack<T: Serialize>(
        _topic: impl AsRef<[u8]>,
        _payload: &T,
    ) -> Result<(), SysError> {
        Err(SysError::ApiError(
            "publish_msgpack is no longer supported: IPC bus carries UTF-8 strings. \
             Use publish_json instead."
                .into(),
        ))
    }

    /// Subscribe to an IPC topic. Returns a typed handle for polling/receiving.
    pub fn subscribe(topic: impl AsRef<[u8]>) -> Result<SubscriptionHandle, SysError> {
        let topic_str = bytes_to_str(topic.as_ref())?;
        let handle_id = wit_ipc::ipc_subscribe(topic_str).map_err(SysError::HostError)?;
        Ok(SubscriptionHandle(handle_id))
    }

    pub fn unsubscribe(handle: &SubscriptionHandle) -> Result<(), SysError> {
        wit_ipc::ipc_unsubscribe(handle.0).map_err(SysError::HostError)
    }

    pub fn poll_bytes(handle: &SubscriptionHandle) -> Result<Vec<u8>, SysError> {
        let envelope = wit_ipc::ipc_poll(handle.0).map_err(SysError::HostError)?;
        // Serialize the envelope back to JSON bytes for backwards compatibility
        // with callers that expect raw JSON envelope bytes.
        let json = envelope_to_json(&envelope)?;
        Ok(json)
    }

    /// Block until a message arrives on a subscription handle, or timeout.
    ///
    /// Returns the message envelope (same format as `poll_bytes`), or an
    /// empty-messages envelope if the timeout expires with no messages.
    /// Max timeout is capped at 60 000 ms by the host.
    pub fn recv_bytes(handle: &SubscriptionHandle, timeout_ms: u64) -> Result<Vec<u8>, SysError> {
        let envelope = wit_ipc::ipc_recv(handle.0, timeout_ms).map_err(SysError::HostError)?;
        let json = envelope_to_json(&envelope)?;
        Ok(json)
    }

    /// Convert a WIT IpcEnvelope to JSON bytes for backward compatibility.
    fn envelope_to_json(envelope: &wit_types::IpcEnvelope) -> Result<Vec<u8>, SysError> {
        #[derive(Serialize)]
        struct JsonEnvelope<'a> {
            messages: Vec<JsonMessage<'a>>,
            dropped: u64,
            lagged: u64,
        }
        #[derive(Serialize)]
        struct JsonMessage<'a> {
            topic: &'a str,
            payload: &'a str,
            source_id: &'a str,
        }

        let json_envelope = JsonEnvelope {
            messages: envelope
                .messages
                .iter()
                .map(|m| JsonMessage {
                    topic: &m.topic,
                    payload: &m.payload,
                    source_id: &m.source_id,
                })
                .collect(),
            dropped: envelope.dropped,
            lagged: envelope.lagged,
        };
        serde_json::to_vec(&json_envelope).map_err(SysError::from)
    }
}

/// Direct frontend messaging (uplinks to CLI, Telegram, etc.).
pub mod uplink {
    use super::*;

    /// An opaque uplink connection identifier. Returned by [`register`].
    #[derive(Debug, Clone)]
    pub struct UplinkId(pub(crate) String);

    impl UplinkId {
        /// Raw ID bytes for interop with lower-level APIs.
        #[must_use]
        pub fn as_bytes(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }

    impl AsRef<[u8]> for UplinkId {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }

    /// Register a new uplink connection. Returns a typed [`UplinkId`].
    pub fn register(
        name: impl AsRef<[u8]>,
        platform: impl AsRef<[u8]>,
        profile: impl AsRef<[u8]>,
    ) -> Result<UplinkId, SysError> {
        let name_str = bytes_to_str(name.as_ref())?;
        let platform_str = bytes_to_str(platform.as_ref())?;
        let profile_str = bytes_to_str(profile.as_ref())?;
        let id = wit_uplink::uplink_register(name_str, platform_str, profile_str)
            .map_err(SysError::HostError)?;
        Ok(UplinkId(id))
    }

    /// Send bytes to a user via an uplink.
    pub fn send_bytes(
        uplink_id: &UplinkId,
        platform_user_id: impl AsRef<[u8]>,
        content: &[u8],
    ) -> Result<Vec<u8>, SysError> {
        let platform_user_str = bytes_to_str(platform_user_id.as_ref())?;
        let content_str = bytes_to_str(content)?;
        let sent = wit_uplink::uplink_send(&uplink_id.0, platform_user_str, content_str)
            .map_err(SysError::HostError)?;
        // Return a JSON-encoded boolean for backward compatibility.
        let result = serde_json::to_vec(&sent)?;
        Ok(result)
    }
}

/// The KV Airlock — Persistent Key-Value Storage
pub mod kv {
    use super::*;

    pub fn get_bytes(key: impl AsRef<[u8]>) -> Result<Vec<u8>, SysError> {
        let key_str = bytes_to_str(key.as_ref())?;
        let result = wit_kv::kv_get(key_str).map_err(SysError::HostError)?;
        Ok(result.unwrap_or_default())
    }

    pub fn set_bytes(key: impl AsRef<[u8]>, value: &[u8]) -> Result<(), SysError> {
        let key_str = bytes_to_str(key.as_ref())?;
        wit_kv::kv_set(key_str, value).map_err(SysError::HostError)
    }

    pub fn get_json<T: DeserializeOwned>(key: impl AsRef<[u8]>) -> Result<T, SysError> {
        let bytes = get_bytes(key)?;
        let parsed = serde_json::from_slice(&bytes)?;
        Ok(parsed)
    }

    pub fn set_json<T: Serialize>(key: impl AsRef<[u8]>, value: &T) -> Result<(), SysError> {
        let bytes = serde_json::to_vec(value)?;
        set_bytes(key, &bytes)
    }

    /// Delete a key from the KV store.
    ///
    /// This is idempotent: deleting a non-existent key succeeds silently.
    /// The underlying store returns whether the key existed, but that
    /// information is not surfaced through the WASM host boundary.
    pub fn delete(key: impl AsRef<[u8]>) -> Result<(), SysError> {
        let key_str = bytes_to_str(key.as_ref())?;
        wit_kv::kv_delete(key_str).map_err(SysError::HostError)
    }

    /// List all keys matching a prefix.
    ///
    /// Returns an empty vec if no keys match. The prefix is matched
    /// against key names within the capsule's scoped namespace.
    pub fn list_keys(prefix: impl AsRef<[u8]>) -> Result<Vec<String>, SysError> {
        let prefix_str = bytes_to_str(prefix.as_ref())?;
        wit_kv::kv_list_keys(prefix_str).map_err(SysError::HostError)
    }

    /// Delete all keys matching a prefix.
    ///
    /// Returns the number of keys deleted. The prefix is matched
    /// against key names within the capsule's scoped namespace.
    pub fn clear_prefix(prefix: impl AsRef<[u8]>) -> Result<u64, SysError> {
        let prefix_str = bytes_to_str(prefix.as_ref())?;
        wit_kv::kv_clear_prefix(prefix_str).map_err(SysError::HostError)
    }

    pub fn get_borsh<T: BorshDeserialize>(key: impl AsRef<[u8]>) -> Result<T, SysError> {
        let bytes = get_bytes(key)?;
        let parsed = borsh::from_slice(&bytes)?;
        Ok(parsed)
    }

    pub fn set_borsh<T: BorshSerialize>(key: impl AsRef<[u8]>, value: &T) -> Result<(), SysError> {
        let bytes = borsh::to_vec(value)?;
        set_bytes(key, &bytes)
    }

    // ---- Versioned KV helpers ----

    /// Internal envelope for versioned KV data.
    ///
    /// Wire format: `{"__sv": <version>, "data": <payload>}`.
    /// The `__sv` prefix is deliberately ugly to avoid collision with
    /// user struct fields.
    #[derive(Serialize, Deserialize)]
    struct VersionedEnvelope<T> {
        #[serde(rename = "__sv")]
        schema_version: u32,
        data: T,
    }

    /// Result of reading versioned data from KV.
    #[derive(Debug)]
    pub enum Versioned<T> {
        /// Data is at the expected schema version.
        Current(T),
        /// Data is at an older version and needs migration.
        NeedsMigration {
            /// Raw JSON value of the `data` field.
            raw: serde_json::Value,
            /// The schema version that was stored.
            stored_version: u32,
        },
        /// Key exists but data has no version envelope (pre-versioning legacy data).
        Unversioned(serde_json::Value),
        /// Key does not exist in KV.
        NotFound,
    }

    /// Write versioned data to KV, wrapped in a schema-version envelope.
    ///
    /// The stored JSON looks like `{"__sv": 1, "data": { ... }}`.
    /// Use [`get_versioned`] or [`get_versioned_or_migrate`] to read it back.
    pub fn set_versioned<T: Serialize>(
        key: impl AsRef<[u8]>,
        value: &T,
        version: u32,
    ) -> Result<(), SysError> {
        let envelope = VersionedEnvelope {
            schema_version: version,
            data: value,
        };
        set_json(key, &envelope)
    }

    /// Read versioned data from KV.
    ///
    /// Returns [`Versioned::Current`] if the stored version matches
    /// `current_version`. Returns [`Versioned::NeedsMigration`] for older
    /// versions. Returns an error for versions newer than `current_version`
    /// (fail secure - don't silently interpret data from a schema you don't
    /// understand).
    ///
    /// Data written by plain [`set_json`] (no envelope) returns
    /// [`Versioned::Unversioned`].
    pub fn get_versioned<T: DeserializeOwned>(
        key: impl AsRef<[u8]>,
        current_version: u32,
    ) -> Result<Versioned<T>, SysError> {
        let bytes = get_bytes(&key)?;
        parse_versioned(&bytes, current_version)
    }

    /// Core parsing logic for versioned KV data, separated from FFI for
    /// testability. Operates on raw bytes as returned by `get_bytes`.
    fn parse_versioned<T: DeserializeOwned>(
        bytes: &[u8],
        current_version: u32,
    ) -> Result<Versioned<T>, SysError> {
        // The host function `kv_get` returns an empty slice when the
        // key is absent. A present key written via set_json/set_versioned
        // always has at least the JSON envelope bytes, so empty = not found.
        if bytes.is_empty() {
            return Ok(Versioned::NotFound);
        }

        let mut value: serde_json::Value = serde_json::from_slice(bytes)?;

        // Detect envelope by checking for __sv (u64) + data fields.
        // If __sv is present but malformed (not a number, or missing data),
        // return an error rather than silently treating as unversioned.
        let sv_field = value.get("__sv");
        let has_sv = sv_field.is_some();
        let envelope_version = sv_field.and_then(|v| v.as_u64());
        let has_data = value.get("data").is_some();

        match (has_sv, envelope_version, has_data) {
            // Valid envelope: __sv is a u64 and data is present.
            // Take ownership of the data field via remove() to avoid cloning.
            (_, Some(v), true) => {
                let v = u32::try_from(v)
                    .map_err(|_| SysError::ApiError("schema version exceeds u32::MAX".into()))?;
                // Safety: the match guard confirmed has_data=true, so
                // value is an object with a "data" key. This is infallible.
                let data = value
                    .as_object_mut()
                    .and_then(|m| m.remove("data"))
                    .expect("data field guaranteed by match condition");
                if v == current_version {
                    let parsed: T = serde_json::from_value(data)?;
                    Ok(Versioned::Current(parsed))
                } else if v < current_version {
                    Ok(Versioned::NeedsMigration {
                        raw: data,
                        stored_version: v,
                    })
                } else {
                    Err(SysError::ApiError(format!(
                        "stored schema version {v} is newer than current \
                         version {current_version} - cannot safely read"
                    )))
                }
            }
            // Malformed envelope: __sv present but data missing or __sv not a number.
            (true, _, _) => Err(SysError::ApiError(
                "malformed versioned envelope: __sv field present but \
                 data field missing or __sv is not a number"
                    .into(),
            )),
            // No __sv field at all: plain unversioned data.
            (false, _, _) => Ok(Versioned::Unversioned(value)),
        }
    }

    /// Read versioned data, automatically migrating older versions.
    ///
    /// `migrate_fn` receives the raw JSON and the stored version, and must
    /// return a `T` at `current_version`. The migrated value is automatically
    /// saved back to KV.
    ///
    /// **Warning:** The original data is overwritten after a successful
    /// migration. If the write-back fails, the original data is preserved
    /// and the migration will be re-attempted on the next call. Ensure
    /// `migrate_fn` is idempotent and correct - there is no rollback
    /// after a successful write.
    ///
    /// For [`Versioned::Unversioned`] data, `migrate_fn` is called with
    /// version 0. For [`Versioned::NotFound`], returns `None`.
    pub fn get_versioned_or_migrate<T: Serialize + DeserializeOwned>(
        key: impl AsRef<[u8]>,
        current_version: u32,
        migrate_fn: impl FnOnce(serde_json::Value, u32) -> Result<T, SysError>,
    ) -> Result<Option<T>, SysError> {
        let key = key.as_ref();

        match get_versioned::<T>(key, current_version)? {
            Versioned::Current(data) => Ok(Some(data)),
            Versioned::NeedsMigration {
                raw,
                stored_version,
            } => {
                let migrated = migrate_fn(raw, stored_version)?;
                set_versioned(key, &migrated, current_version)?;
                Ok(Some(migrated))
            }
            Versioned::Unversioned(raw) => {
                let migrated = migrate_fn(raw, 0)?;
                set_versioned(key, &migrated, current_version)?;
                Ok(Some(migrated))
            }
            Versioned::NotFound => Ok(None),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestData {
            name: String,
            count: u32,
        }

        // ---- Envelope serialization tests ----

        #[test]
        fn versioned_envelope_roundtrip() {
            let envelope = VersionedEnvelope {
                schema_version: 1,
                data: TestData {
                    name: "hello".into(),
                    count: 42,
                },
            };
            let json = serde_json::to_string(&envelope).unwrap();
            assert!(json.contains("\"__sv\":1"));
            assert!(json.contains("\"data\":{"));

            let parsed: VersionedEnvelope<TestData> = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.schema_version, 1);
            assert_eq!(
                parsed.data,
                TestData {
                    name: "hello".into(),
                    count: 42,
                }
            );
        }

        #[test]
        fn versioned_envelope_wire_format() {
            let envelope = VersionedEnvelope {
                schema_version: 3,
                data: serde_json::json!({"key": "value"}),
            };
            let json = serde_json::to_string(&envelope).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

            assert_eq!(parsed["__sv"], 3);
            assert_eq!(parsed["data"]["key"], "value");
        }

        // ---- parse_versioned logic tests ----

        #[test]
        fn parse_versioned_empty_bytes_returns_not_found() {
            let result = parse_versioned::<TestData>(b"", 1).unwrap();
            assert!(matches!(result, Versioned::NotFound));
        }

        #[test]
        fn parse_versioned_current_version_returns_current() {
            let bytes = br#"{"__sv":2,"data":{"name":"hello","count":42}}"#;
            let result = parse_versioned::<TestData>(bytes, 2).unwrap();
            match result {
                Versioned::Current(data) => {
                    assert_eq!(data.name, "hello");
                    assert_eq!(data.count, 42);
                }
                other => panic!("expected Current, got {other:?}"),
            }
        }

        #[test]
        fn parse_versioned_older_version_returns_needs_migration() {
            let bytes = br#"{"__sv":1,"data":{"name":"old","count":1}}"#;
            let result = parse_versioned::<TestData>(bytes, 3).unwrap();
            match result {
                Versioned::NeedsMigration {
                    raw,
                    stored_version,
                } => {
                    assert_eq!(stored_version, 1);
                    assert_eq!(raw["name"], "old");
                    assert_eq!(raw["count"], 1);
                }
                other => panic!("expected NeedsMigration, got {other:?}"),
            }
        }

        #[test]
        fn parse_versioned_newer_version_returns_error() {
            let bytes = br#"{"__sv":5,"data":{"name":"future","count":0}}"#;
            let result = parse_versioned::<TestData>(bytes, 2);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("newer than current"),
                "error should mention newer version: {err}"
            );
        }

        #[test]
        fn parse_versioned_plain_json_returns_unversioned() {
            let bytes = br#"{"name":"legacy","count":99}"#;
            let result = parse_versioned::<TestData>(bytes, 1).unwrap();
            match result {
                Versioned::Unversioned(val) => {
                    assert_eq!(val["name"], "legacy");
                    assert_eq!(val["count"], 99);
                }
                other => panic!("expected Unversioned, got {other:?}"),
            }
        }

        #[test]
        fn parse_versioned_malformed_sv_without_data_returns_error() {
            let bytes = br#"{"__sv":1,"payload":"something"}"#;
            let result = parse_versioned::<TestData>(bytes, 1);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("malformed"),
                "error should mention malformed envelope: {err}"
            );
        }

        #[test]
        fn parse_versioned_non_numeric_sv_returns_error() {
            let bytes = br#"{"__sv":"one","data":{}}"#;
            let result = parse_versioned::<TestData>(bytes, 1);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("malformed"),
                "error should mention malformed envelope: {err}"
            );
        }

        #[test]
        fn parse_versioned_version_zero_is_valid() {
            // Version 0 is a legitimate version (initial schema).
            let bytes = br#"{"__sv":0,"data":{"name":"v0","count":0}}"#;
            let result = parse_versioned::<TestData>(bytes, 0).unwrap();
            assert!(matches!(result, Versioned::Current(_)));
        }

        #[test]
        fn parse_versioned_invalid_json_returns_error() {
            let result = parse_versioned::<TestData>(b"not json", 1);
            assert!(result.is_err());
        }
    }
}

/// The HTTP Airlock — External Network Requests
/// Outbound HTTP — typed request API over the host HTTP airlock.
pub mod http {
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
    #[derive(Debug)]
    pub struct Response {
        bytes: Vec<u8>,
    }

    impl Response {
        /// The raw response body as bytes.
        pub fn bytes(&self) -> &[u8] {
            &self.bytes
        }

        /// The response body as a UTF-8 string.
        pub fn text(&self) -> Result<&str, SysError> {
            core::str::from_utf8(&self.bytes).map_err(|e| SysError::ApiError(e.to_string()))
        }

        /// Deserialize the response body as JSON.
        pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, SysError> {
            serde_json::from_slice(&self.bytes).map_err(SysError::from)
        }
    }

    /// Send an HTTP request and wait for the full response.
    pub fn send(request: &Request) -> Result<Response, SysError> {
        let wit_req = request.to_wit();
        let result = wit_http::http_request(&wit_req).map_err(SysError::HostError)?;
        Ok(Response {
            bytes: result.body.into_bytes(),
        })
    }

    /// Represents an active streaming HTTP response.
    ///
    /// Must be explicitly closed via [`stream_close`] when done.
    /// Not `Clone` — each handle is a unique owner of the host-side resource.
    #[derive(Debug)]
    pub struct HttpStreamHandle(u64);

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
}

/// Capsule configuration (like `std::env`).
///
/// In the Astrid model, capsule config entries are the equivalent of
/// environment variables. The kernel injects them at load time.
pub mod env {
    use super::*;

    /// Well-known config key for the kernel's Unix domain socket path.
    pub const CONFIG_SOCKET_PATH: &str = "ASTRID_SOCKET_PATH";

    /// Read a config value as raw bytes. Like `std::env::var_os`.
    pub fn var_bytes(key: impl AsRef<[u8]>) -> Result<Vec<u8>, SysError> {
        let key_str = bytes_to_str(key.as_ref())?;
        let result = wit_sys::get_config(key_str).map_err(SysError::HostError)?;
        Ok(result.into_bytes())
    }

    /// Read a config value as a UTF-8 string. Like `std::env::var`.
    pub fn var(key: impl AsRef<[u8]>) -> Result<String, SysError> {
        let key_str = bytes_to_str(key.as_ref())?;
        wit_sys::get_config(key_str).map_err(SysError::HostError)
    }
}

/// Wall-clock access — mirrors [`std::time`].
///
/// The WASM guest has no direct access to system time. All calls go
/// through the host. Returns [`std::time::SystemTime`] for compatibility
/// with standard Rust code.
pub mod time {
    use super::*;

    /// Returns the current wall-clock time.
    ///
    /// This is a host call — the WASM guest has no direct access to the
    /// system clock. Unlike [`std::time::SystemTime::now`], this returns
    /// `Result` because the host call can fail.
    pub fn now() -> Result<std::time::SystemTime, SysError> {
        let ms = wit_sys::clock_ms();
        Ok(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms))
    }
}

/// Structured logging — mirrors the [`log`](https://docs.rs/log) crate conventions.
pub mod log {
    use super::*;
    use core::fmt::Display;

    /// Log a message at the given level.
    pub fn log(level: &str, message: impl Display) -> Result<(), SysError> {
        let msg = format!("{message}");
        let wit_level = match level {
            "trace" => wit_types::LogLevel::Trace,
            "debug" => wit_types::LogLevel::Debug,
            "info" => wit_types::LogLevel::Info,
            "warn" => wit_types::LogLevel::Warn,
            "error" => wit_types::LogLevel::Error,
            _ => wit_types::LogLevel::Info,
        };
        wit_sys::log(wit_level, &msg);
        Ok(())
    }

    /// Log at DEBUG level.
    pub fn debug(message: impl Display) -> Result<(), SysError> {
        log("debug", message)
    }

    /// Log at INFO level.
    pub fn info(message: impl Display) -> Result<(), SysError> {
        log("info", message)
    }

    /// Log at WARN level.
    pub fn warn(message: impl Display) -> Result<(), SysError> {
        log("warn", message)
    }

    /// Log at ERROR level.
    pub fn error(message: impl Display) -> Result<(), SysError> {
        log("error", message)
    }
}

/// OS runtime introspection and signaling.
pub mod runtime {
    use super::*;

    /// Signal that the capsule's run loop is ready.
    ///
    /// Call this after setting up IPC subscriptions in `run()` to let the
    /// kernel know this capsule is ready to receive events. The kernel waits
    /// for this signal before loading dependent capsules.
    pub fn signal_ready() -> Result<(), SysError> {
        wit_sys::signal_ready();
        Ok(())
    }

    /// Retrieves the caller context (User ID and Session ID) for the current execution.
    pub fn caller() -> Result<crate::types::CallerContext, SysError> {
        let ctx = wit_sys::get_caller().map_err(SysError::HostError)?;
        Ok(crate::types::CallerContext {
            session_id: Some(ctx.source_id),
            user_id: ctx.principal,
        })
    }

    /// Returns the kernel's Unix domain socket path.
    ///
    /// Reads from the well-known `ASTRID_SOCKET_PATH` config key that the
    /// kernel injects into every capsule at load time.
    pub fn socket_path() -> Result<String, SysError> {
        let raw = crate::env::var(crate::env::CONFIG_SOCKET_PATH)?;
        // var() returns config values directly (no JSON encoding with WIT).
        // Use proper JSON parsing to handle escape sequences correctly.
        let path = serde_json::from_str::<String>(raw.trim()).or_else(|_| {
            // Fallback: if the value isn't valid JSON, use it raw.
            if raw.is_empty() {
                Err(SysError::ApiError(
                    "ASTRID_SOCKET_PATH config key is empty".to_string(),
                ))
            } else {
                Ok(raw)
            }
        })?;
        // Reject paths with null bytes - they would silently truncate at the OS level.
        if path.contains('\0') {
            return Err(SysError::ApiError(
                "ASTRID_SOCKET_PATH contains null byte".to_string(),
            ));
        }
        Ok(path)
    }
}

/// The Hooks Airlock — Executing User Middleware
pub mod hooks {
    use super::*;

    pub fn trigger(event_bytes: &[u8]) -> Result<Vec<u8>, SysError> {
        let request_json = bytes_to_str(event_bytes)?;
        let result = wit_sys::trigger_hook(request_json).map_err(SysError::HostError)?;
        Ok(result.into_bytes())
    }
}

/// Cross-capsule capability queries.
///
/// Allows a capsule to check whether another capsule (identified by its
/// IPC session UUID) has a specific manifest capability. Used by the
/// prompt builder to enforce `allow_prompt_injection` gating.
pub mod capabilities {
    use super::*;

    /// Check whether a capsule has a specific capability.
    ///
    /// Returns `true` if the capsule identified by `source_uuid` has the
    /// given `capability` declared in its manifest. Returns `false` for
    /// unknown UUIDs, unknown capabilities, or on any error (fail-closed).
    pub fn check(source_uuid: &str, capability: &str) -> Result<bool, SysError> {
        let request = wit_types::CapabilityCheckRequest {
            source_uuid: source_uuid.to_string(),
            capability: capability.to_string(),
        };
        let response = wit_sys::check_capsule_capability(&request).map_err(SysError::HostError)?;
        Ok(response.allowed)
    }
}

pub mod net;
pub mod process {
    use super::*;
    use serde::{Deserialize, Serialize};

    /// Request payload for spawning a host process.
    #[derive(Debug, Serialize)]
    pub struct ProcessRequest<'a> {
        pub cmd: &'a str,
        pub args: &'a [&'a str],
    }

    /// Result returned from a spawned host process.
    #[derive(Debug, Deserialize)]
    pub struct ProcessResult {
        pub stdout: String,
        pub stderr: String,
        pub exit_code: i32,
    }

    /// Spawns a native host process (blocks until completion).
    /// The Capsule must have the `host_process` capability granted for this command.
    pub fn spawn(cmd: &str, args: &[&str]) -> Result<ProcessResult, SysError> {
        let request = wit_types::SpawnRequest {
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        };
        let result = wit_process::spawn(&request).map_err(SysError::HostError)?;
        Ok(ProcessResult {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
        })
    }

    // -------------------------------------------------------------------
    // Background process management
    // -------------------------------------------------------------------

    /// Handle returned when a background process is spawned.
    #[derive(Debug, Deserialize)]
    pub struct BackgroundProcessHandle {
        /// Opaque handle ID (not an OS PID).
        id: u64,
    }

    impl BackgroundProcessHandle {
        /// Returns the opaque handle ID for this process.
        pub fn id(&self) -> u64 {
            self.id
        }
    }

    /// Buffered logs and status from a background process.
    #[derive(Debug, Deserialize)]
    pub struct ProcessLogs {
        /// New stdout output since the last read.
        pub stdout: String,
        /// New stderr output since the last read.
        pub stderr: String,
        /// Whether the process is still running.
        pub running: bool,
        /// Exit code if the process has exited.
        pub exit_code: Option<i32>,
    }

    /// Result from killing a background process.
    #[derive(Debug, Deserialize)]
    pub struct KillResult {
        /// Whether the process was successfully killed.
        pub killed: bool,
        /// Exit code of the terminated process.
        pub exit_code: Option<i32>,
        /// Any remaining buffered stdout.
        pub stdout: String,
        /// Any remaining buffered stderr.
        pub stderr: String,
    }

    /// Spawn a background host process.
    ///
    /// Returns an opaque handle that can be used with [`read_logs`] and
    /// [`kill`]. The process runs sandboxed with piped stdout/stderr.
    pub fn spawn_background(cmd: &str, args: &[&str]) -> Result<BackgroundProcessHandle, SysError> {
        let request = wit_types::SpawnRequest {
            cmd: cmd.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        };
        let result = wit_process::spawn_background(&request).map_err(SysError::HostError)?;
        Ok(BackgroundProcessHandle { id: result.id })
    }

    /// Read buffered output from a background process.
    ///
    /// Each call drains the buffer and returns only NEW output since the
    /// last read. Also reports whether the process is still running.
    pub fn read_logs(id: u64) -> Result<ProcessLogs, SysError> {
        let result = wit_process::read_logs(id).map_err(SysError::HostError)?;
        Ok(ProcessLogs {
            stdout: result.stdout,
            stderr: result.stderr,
            running: result.running,
            exit_code: result.exit_code,
        })
    }

    /// Kill a background process and release its resources.
    ///
    /// Returns any remaining buffered output along with the exit code.
    pub fn kill(id: u64) -> Result<KillResult, SysError> {
        let result = wit_process::kill(id).map_err(SysError::HostError)?;
        Ok(KillResult {
            killed: result.killed,
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
        })
    }
}

/// The Elicit Airlock - User Input During Install/Upgrade Lifecycle
///
/// These functions are only callable during `#[astrid::install]` and
/// `#[astrid::upgrade]` hooks. Calling them from a tool or interceptor
/// returns a host error.
pub mod elicit {
    use super::*;

    /// Validates that the elicit key is non-empty and not whitespace-only.
    fn validate_key(key: &str) -> Result<(), SysError> {
        if key.trim().is_empty() {
            return Err(SysError::ApiError("elicit key must not be empty".into()));
        }
        Ok(())
    }

    /// Store a secret via the kernel's `SecretStore`. The capsule **never**
    /// receives the value. Returns `Ok(())` confirming the user provided it.
    pub fn secret(key: &str, description: &str) -> Result<(), SysError> {
        validate_key(key)?;
        let req = wit_types::ElicitRequest {
            elicit_type: "secret".to_string(),
            key: key.to_string(),
            description: description.to_string(),
            options: None,
            default_value: None,
        };
        let resp_str = wit_elicit::elicit(&req).map_err(SysError::HostError)?;

        #[derive(serde::Deserialize)]
        struct SecretResp {
            ok: bool,
        }
        let resp: SecretResp = serde_json::from_str(&resp_str)?;
        if !resp.ok {
            return Err(SysError::ApiError(
                "kernel did not confirm secret storage".into(),
            ));
        }
        Ok(())
    }

    /// Check if a secret has been configured (without reading it).
    pub fn has_secret(key: &str) -> Result<bool, SysError> {
        validate_key(key)?;
        wit_elicit::has_secret(key).map_err(SysError::HostError)
    }

    /// Shared implementation for text elicitation with optional default.
    fn elicit_text(
        key: &str,
        description: &str,
        default: Option<&str>,
    ) -> Result<String, SysError> {
        validate_key(key)?;
        let req = wit_types::ElicitRequest {
            elicit_type: "text".to_string(),
            key: key.to_string(),
            description: description.to_string(),
            options: None,
            default_value: default.map(|s| s.to_string()),
        };
        let resp_str = wit_elicit::elicit(&req).map_err(SysError::HostError)?;

        #[derive(serde::Deserialize)]
        struct TextResp {
            value: String,
        }
        let resp: TextResp = serde_json::from_str(&resp_str)?;
        Ok(resp.value)
    }

    /// Prompt for a text value. Blocks until the user responds.
    /// Use [`secret()`] for sensitive data - this returns the value to the capsule.
    pub fn text(key: &str, description: &str) -> Result<String, SysError> {
        elicit_text(key, description, None)
    }

    /// Prompt with a default value pre-filled.
    pub fn text_with_default(
        key: &str,
        description: &str,
        default: &str,
    ) -> Result<String, SysError> {
        elicit_text(key, description, Some(default))
    }

    /// Prompt for a selection from a list. Returns the selected value.
    pub fn select(key: &str, description: &str, options: &[&str]) -> Result<String, SysError> {
        validate_key(key)?;
        if options.is_empty() {
            return Err(SysError::ApiError(
                "select requires at least one option".into(),
            ));
        }
        let req = wit_types::ElicitRequest {
            elicit_type: "select".to_string(),
            key: key.to_string(),
            description: description.to_string(),
            options: Some(options.iter().map(|s| s.to_string()).collect()),
            default_value: None,
        };
        let resp_str = wit_elicit::elicit(&req).map_err(SysError::HostError)?;

        #[derive(serde::Deserialize)]
        struct SelectResp {
            value: String,
        }
        let resp: SelectResp = serde_json::from_str(&resp_str)?;
        if !options.iter().any(|o| *o == resp.value) {
            let truncated: String = resp.value.chars().take(64).collect();
            return Err(SysError::ApiError(format!(
                "host returned value '{truncated}' not in provided options",
            )));
        }
        Ok(resp.value)
    }

    /// Prompt for multiple text values (array input).
    pub fn array(key: &str, description: &str) -> Result<Vec<String>, SysError> {
        validate_key(key)?;
        let req = wit_types::ElicitRequest {
            elicit_type: "array".to_string(),
            key: key.to_string(),
            description: description.to_string(),
            options: None,
            default_value: None,
        };
        let resp_str = wit_elicit::elicit(&req).map_err(SysError::HostError)?;

        #[derive(serde::Deserialize)]
        struct ArrayResp {
            values: Vec<String>,
        }
        let resp: ArrayResp = serde_json::from_str(&resp_str)?;
        Ok(resp.values)
    }
}

/// Auto-subscribed interceptor bindings for run-loop capsules.
///
/// When a capsule declares both `run()` and `[[interceptor]]`, the runtime
/// auto-subscribes to each interceptor's topic and delivers events through
/// the IPC channel the run loop already reads from. This module provides
/// helpers to query the subscription mappings and dispatch events by action.
pub mod interceptors {
    use super::*;

    /// A single interceptor subscription binding.
    #[derive(Debug)]
    pub struct InterceptorBinding {
        /// The IPC subscription handle ID.
        pub handle_id: u64,
        /// The interceptor action name from the manifest.
        pub action: String,
        /// The event topic this interceptor subscribes to.
        pub topic: String,
    }

    impl InterceptorBinding {
        /// Return a subscription handle for use with `ipc::poll_bytes` / `ipc::recv_bytes`.
        #[must_use]
        pub fn subscription_handle(&self) -> ipc::SubscriptionHandle {
            ipc::SubscriptionHandle(self.handle_id)
        }

        /// Return the raw handle ID bytes (for lower-level interop).
        #[must_use]
        pub fn handle_bytes(&self) -> Vec<u8> {
            self.handle_id.to_string().into_bytes()
        }
    }

    /// Query the runtime for auto-subscribed interceptor handles.
    ///
    /// Returns an empty vec if this capsule has no auto-subscribed interceptors
    /// (i.e. it does not have both `run()` and `[[interceptor]]`).
    pub fn bindings() -> Result<Vec<InterceptorBinding>, SysError> {
        let handles = wit_ipc::get_interceptor_handles().map_err(SysError::HostError)?;
        Ok(handles
            .into_iter()
            .map(|h| InterceptorBinding {
                handle_id: h.handle_id,
                action: h.action,
                topic: h.topic,
            })
            .collect())
    }

    /// Poll all interceptor subscriptions and dispatch pending events.
    ///
    /// For each binding with pending messages, calls
    /// `handler(action, envelope_bytes)` once with the full batch envelope
    /// (JSON with `messages` array, `dropped`, and `lagged` fields).
    /// Bindings with no pending messages are skipped.
    pub fn poll(
        bindings: &[InterceptorBinding],
        mut handler: impl FnMut(&str, &[u8]),
    ) -> Result<(), SysError> {
        for binding in bindings {
            let handle = binding.subscription_handle();
            let envelope = ipc::poll_bytes(&handle)?;

            // poll_bytes always returns a JSON envelope like
            // `{"messages":[],"dropped":0,"lagged":0}`. Check the
            // messages array before calling the handler.
            #[derive(serde::Deserialize)]
            struct PollEnvelope {
                messages: Vec<serde_json::Value>,
            }
            let parsed: PollEnvelope = serde_json::from_slice(&envelope)?;
            if !parsed.messages.is_empty() {
                handler(&binding.action, &envelope);
            }
        }
        Ok(())
    }
}

/// Request human approval for sensitive actions from within a capsule.
///
/// Any capsule can call [`approval::request`] to block until the frontend
/// user approves or denies an action. The host function checks the
/// `AllowanceStore` for a matching pattern first (instant path), and only
/// prompts the user when no allowance exists.
///
/// # Example
///
/// ```ignore
/// use astrid_sdk::prelude::*;
///
/// let result = approval::request("git push", "git push origin main", "high")?;
/// if !result.approved {
///     return Err(SysError::ApiError("Action denied by user".into()));
/// }
/// ```
/// Platform identity resolution and linking.
///
/// Capsules use this module to resolve platform-specific user identities
/// (e.g. Discord user IDs, Twitch usernames) to Astrid-native user IDs,
/// and to manage the links between them.
///
/// Requires the `identity` capability in `Capsule.toml`:
/// - `["resolve"]` - resolve platform users
/// - `["link"]` - resolve, link, unlink, and list links
/// - `["admin"]` - all of the above plus create new users
pub mod identity {
    use super::*;

    /// A resolved Astrid user returned by [`resolve`].
    #[derive(Debug)]
    pub struct ResolvedUser {
        /// The Astrid-native user ID (UUID).
        pub user_id: String,
        /// Optional display name.
        pub display_name: Option<String>,
    }

    /// A platform-to-Astrid identity link.
    #[derive(Debug)]
    pub struct Link {
        /// Platform name (e.g. "discord", "twitch").
        pub platform: String,
        /// Platform-specific user identifier.
        pub platform_user_id: String,
        /// The Astrid user this is linked to.
        pub astrid_user_id: String,
        /// When the link was created (RFC 3339).
        pub linked_at: String,
        /// How the link was established (e.g. "system", "chat_command").
        pub method: String,
    }

    /// Resolve a platform user to an Astrid user.
    ///
    /// Returns `Ok(Some(user))` if the platform identity is linked,
    /// `Ok(None)` if not found. Requires `identity = ["resolve"]` or higher.
    pub fn resolve(
        platform: &str,
        platform_user_id: &str,
    ) -> Result<Option<ResolvedUser>, SysError> {
        let request = wit_types::IdentityResolveRequest {
            platform: platform.to_string(),
            platform_user_id: platform_user_id.to_string(),
        };
        let resp = wit_identity::identity_resolve(&request).map_err(SysError::HostError)?;
        if resp.found {
            let user_id = resp.user_id.ok_or_else(|| {
                SysError::ApiError("host returned found=true but user_id was missing".into())
            })?;
            Ok(Some(ResolvedUser {
                user_id,
                display_name: resp.display_name,
            }))
        } else if let Some(err) = resp.error {
            Err(SysError::ApiError(err))
        } else {
            Ok(None)
        }
    }

    /// Link a platform identity to an Astrid user.
    ///
    /// - `method` describes how the link was established (e.g. "chat_command", "system").
    ///
    /// Returns the created link on success. Requires `identity = ["link"]` or higher.
    pub fn link(
        platform: &str,
        platform_user_id: &str,
        astrid_user_id: &str,
        method: &str,
    ) -> Result<Link, SysError> {
        let request = wit_types::IdentityLinkRequest {
            platform: platform.to_string(),
            platform_user_id: platform_user_id.to_string(),
            astrid_user_id: astrid_user_id.to_string(),
            method: method.to_string(),
        };
        let resp = wit_identity::identity_link(&request).map_err(SysError::HostError)?;
        if !resp.ok {
            return Err(SysError::ApiError(
                resp.error.unwrap_or_else(|| "identity link failed".into()),
            ));
        }
        // The WIT identity-ok-response doesn't carry link details directly.
        // Reconstruct the link from the request parameters since the host confirmed success.
        Ok(Link {
            platform: platform.to_string(),
            platform_user_id: platform_user_id.to_string(),
            astrid_user_id: astrid_user_id.to_string(),
            linked_at: String::new(), // Not available from identity-ok-response
            method: method.to_string(),
        })
    }

    /// Unlink a platform identity from its Astrid user.
    ///
    /// Returns `true` if a link was removed, `false` if none existed.
    /// Requires `identity = ["link"]` or higher.
    pub fn unlink(platform: &str, platform_user_id: &str) -> Result<bool, SysError> {
        let request = wit_types::IdentityUnlinkRequest {
            platform: platform.to_string(),
            platform_user_id: platform_user_id.to_string(),
        };
        let resp = wit_identity::identity_unlink(&request).map_err(SysError::HostError)?;
        if !resp.ok {
            return Err(SysError::ApiError(
                resp.error
                    .unwrap_or_else(|| "identity unlink failed".into()),
            ));
        }
        Ok(resp.removed.unwrap_or(false))
    }

    /// Create a new Astrid user.
    ///
    /// Returns the UUID of the newly created user.
    /// Requires `identity = ["admin"]`.
    pub fn create_user(display_name: Option<&str>) -> Result<String, SysError> {
        let request = wit_types::IdentityCreateUserRequest {
            display_name: display_name.map(|s| s.to_string()),
        };
        let resp = wit_identity::identity_create_user(&request).map_err(SysError::HostError)?;
        if !resp.ok {
            return Err(SysError::ApiError(
                resp.error
                    .unwrap_or_else(|| "identity create_user failed".into()),
            ));
        }
        resp.user_id
            .ok_or_else(|| SysError::ApiError("missing user_id in response".into()))
    }

    /// List all platform links for an Astrid user.
    ///
    /// Returns all linked platform identities for the given user UUID.
    /// Requires `identity = ["link"]` or higher.
    pub fn list_links(astrid_user_id: &str) -> Result<Vec<Link>, SysError> {
        let request = wit_types::IdentityListLinksRequest {
            astrid_user_id: astrid_user_id.to_string(),
        };
        let resp = wit_identity::identity_list_links(&request).map_err(SysError::HostError)?;
        if !resp.ok {
            return Err(SysError::ApiError(
                resp.error
                    .unwrap_or_else(|| "identity list_links failed".into()),
            ));
        }
        // Parse links from the links_json field.
        if let Some(json_str) = &resp.links_json {
            #[derive(Deserialize)]
            struct LinkInfo {
                platform: String,
                platform_user_id: String,
                astrid_user_id: String,
                linked_at: String,
                method: String,
            }
            let links: Vec<LinkInfo> = serde_json::from_str(json_str)?;
            Ok(links
                .into_iter()
                .map(|l| Link {
                    platform: l.platform,
                    platform_user_id: l.platform_user_id,
                    astrid_user_id: l.astrid_user_id,
                    linked_at: l.linked_at,
                    method: l.method,
                })
                .collect())
        } else {
            Ok(Vec::new())
        }
    }
}

pub mod approval {
    use super::*;

    /// The result of an approval request.
    #[derive(Debug)]
    pub struct ApprovalResult {
        /// Whether the action was approved.
        pub approved: bool,
        /// The decision string: "approve", "approve_session",
        /// "approve_always", "deny", or "allowance" (auto-approved).
        pub decision: String,
    }

    /// Request human approval for a sensitive action.
    ///
    /// Blocks the capsule until the frontend user responds or the request
    /// times out. If an existing allowance matches, returns immediately
    /// without prompting.
    ///
    /// - `action` - short description of the action (e.g. "git push")
    /// - `resource` - full resource identifier (e.g. "git push origin main")
    /// - `risk_level` - one of "low", "medium", "high", "critical"
    pub fn request(
        action: &str,
        resource: &str,
        risk_level: &str,
    ) -> Result<ApprovalResult, SysError> {
        let req = wit_types::ApprovalRequest {
            action: action.to_string(),
            target_resource: resource.to_string(),
            risk_level: risk_level.to_string(),
        };
        let resp = wit_approval::request_approval(&req).map_err(SysError::HostError)?;
        Ok(ApprovalResult {
            approved: resp.approved,
            decision: resp.decision,
        })
    }
}

pub mod prelude {
    pub use crate::{
        SysError,
        // Astrid-specific modules
        approval,
        capabilities,
        elicit,
        // std-mirrored modules
        env,
        fs,
        hooks,
        http,
        identity,
        interceptors,
        ipc,
        kv,
        log,
        net,
        process,
        runtime,
        time,
        uplink,
    };

    #[cfg(feature = "derive")]
    pub use astrid_sdk_macros::capsule;
}
