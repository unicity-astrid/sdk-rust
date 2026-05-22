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

#![forbid(unsafe_code)]
#![allow(missing_docs)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

// Per-domain WIT (post-PR #752 split). Every host fn lives under
// `astrid::<domain>::host`; the foundation I/O types are under
// `astrid::io::{error, poll, streams}`. The aliases below preserve
// the `wit_<domain>` names the wrappers use so most call sites stay
// touched only in their error-mapping code.
use astrid_sys::astrid::approval::host as wit_approval;
use astrid_sys::astrid::elicit::host as wit_elicit;
use astrid_sys::astrid::fs::host as wit_fs;
use astrid_sys::astrid::http::host as wit_http;
use astrid_sys::astrid::identity::host as wit_identity;
use astrid_sys::astrid::io::error as wit_io_error;
use astrid_sys::astrid::io::poll as wit_io_poll;
use astrid_sys::astrid::io::streams as wit_io_streams;
use astrid_sys::astrid::ipc::host as wit_ipc;
use astrid_sys::astrid::kv::host as wit_kv;
use astrid_sys::astrid::net::host as wit_net;
use astrid_sys::astrid::process::host as wit_process;
use astrid_sys::astrid::sys::host as wit_sys;
use astrid_sys::astrid::uplink::host as wit_uplink;
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

    /// Identifies the caller that triggered the current capsule execution.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CallerContext {
        /// UUID of the capsule that originated the IPC message.
        pub source_id: String,
        /// The acting principal (user ID), if available.
        pub principal: Option<String>,
        /// ISO 8601 timestamp of the originating message.
        pub timestamp: String,
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
    #[error("Borsh serialization error: {0}")]
    BorshError(#[from] std::io::Error),
    #[error("API logic error: {0}")]
    ApiError(String),
}

pub mod fs;

/// Shared IPC event types generated from the canonical WIT contracts.
///
/// These types are the standard payloads for cross-capsule IPC topics
/// (LLM requests, session messages, registry events, etc.). They are
/// generated from the `unicity-astrid/wit` repository which defines
/// the canonical WIT interfaces.
///
/// Capsule authors use these types with [`ipc::publish_json`] instead
/// of defining their own structs for standard topics.
#[cfg(feature = "derive")]
pub mod contracts {
    astrid_sdk_macros::wit_events!("wit/astrid-contracts.wit");
}

/// Event bus messaging (like `std::sync::mpsc` but topic-based).
pub mod ipc;

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
    pub fn register(name: &str, platform: &str, profile: &str) -> Result<UplinkId, SysError> {
        let id =
            wit_uplink::uplink_register(name, platform, profile).map_err(SysError::HostError)?;
        Ok(UplinkId(id))
    }

    /// Send a message to a user via an uplink.
    ///
    /// Returns `true` if sent, `false` if the message was dropped.
    pub fn send(
        uplink_id: &UplinkId,
        platform_user_id: &str,
        content: &str,
    ) -> Result<bool, SysError> {
        wit_uplink::uplink_send(&uplink_id.0, platform_user_id, content)
            .map_err(SysError::HostError)
    }
}

/// The KV Airlock — Persistent Key-Value Storage
pub mod kv;

/// The HTTP Airlock — External Network Requests
/// Outbound HTTP — typed request API over the host HTTP airlock.
pub mod http;

/// Capsule configuration (like `std::env`).
///
/// In the Astrid model, capsule config entries are the equivalent of
/// environment variables. The kernel injects them at load time.
pub mod env {
    use super::*;

    /// Well-known config key for the kernel's Unix domain socket path.
    pub const CONFIG_SOCKET_PATH: &str = "ASTRID_SOCKET_PATH";

    /// Read a config value as raw bytes. Like `std::env::var_os`.
    pub fn var_bytes(key: &str) -> Result<Vec<u8>, SysError> {
        let key_str = key;
        let result = wit_sys::get_config(key_str).map_err(SysError::HostError)?;
        Ok(result.into_bytes())
    }

    /// Read a config value as a UTF-8 string. Like `std::env::var`.
    pub fn var(key: &str) -> Result<String, SysError> {
        let key_str = key;
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
///
/// All functions are infallible — the host `log()` call cannot fail.
pub mod log {
    use super::*;
    use core::fmt::Display;

    /// Log at TRACE level.
    pub fn trace(message: impl Display) {
        wit_sys::log(wit_types::LogLevel::Trace, &format!("{message}"));
    }

    /// Log at DEBUG level.
    pub fn debug(message: impl Display) {
        wit_sys::log(wit_types::LogLevel::Debug, &format!("{message}"));
    }

    /// Log at INFO level.
    pub fn info(message: impl Display) {
        wit_sys::log(wit_types::LogLevel::Info, &format!("{message}"));
    }

    /// Log at WARN level.
    pub fn warn(message: impl Display) {
        wit_sys::log(wit_types::LogLevel::Warn, &format!("{message}"));
    }

    /// Log at ERROR level.
    pub fn error(message: impl Display) {
        wit_sys::log(wit_types::LogLevel::Error, &format!("{message}"));
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
            source_id: ctx.source_id,
            principal: ctx.principal,
            timestamp: ctx.timestamp,
        })
    }

    /// Returns the kernel's Unix domain socket path.
    ///
    /// Reads from the well-known `ASTRID_SOCKET_PATH` config key that the
    /// kernel injects into every capsule at load time.
    pub fn socket_path() -> Result<String, SysError> {
        let path = crate::env::var(crate::env::CONFIG_SOCKET_PATH)?;
        // WIT get-config returns values directly (no JSON encoding).
        if path.is_empty() {
            return Err(SysError::ApiError(
                "ASTRID_SOCKET_PATH config key is empty".to_string(),
            ));
        }
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

    pub fn trigger(event: &str) -> Result<String, SysError> {
        wit_sys::trigger_hook(event).map_err(SysError::HostError)
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
pub mod process;

/// The Elicit Airlock - User Input During Install/Upgrade Lifecycle
///
/// These functions are only callable during `#[astrid::install]` and
/// `#[astrid::upgrade]` hooks. Calling them from a tool or interceptor
/// returns a host error.
pub mod elicit;

/// Auto-subscribed interceptor bindings for run-loop capsules.
///
/// When a capsule declares both `run()` and `[[interceptor]]`, the runtime
/// auto-subscribes to each interceptor's topic and delivers events through
/// the IPC channel the run loop already reads from. This module provides
/// helpers to query the subscription mappings and dispatch events by action.
pub mod interceptors;

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
/// if !approval::request("git push", "git push origin main")? {
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
pub mod identity;

/// Human-in-the-loop approval for sensitive actions.
///
/// The capsule declares what it wants to do. The kernel classifies risk
/// and manages allowances internally — the capsule sees only approved/denied.
/// This follows the OS permission model: user space requests, the system decides.
pub mod approval {
    use super::*;

    /// Request human approval for a sensitive action.
    ///
    /// Blocks the capsule until the frontend user responds or the request
    /// times out. If an existing allowance matches, returns immediately
    /// without prompting.
    ///
    /// Returns `true` if approved, `false` if denied.
    ///
    /// # Example
    /// ```ignore
    /// if approval::request("git push", "git push origin main")? {
    ///     // proceed with the push
    /// }
    /// ```
    pub fn request(action: &str, resource: &str) -> Result<bool, SysError> {
        let req = wit_types::ApprovalRequest {
            action: action.to_string(),
            target_resource: resource.to_string(),
        };
        let resp = wit_approval::request_approval(&req).map_err(SysError::HostError)?;
        Ok(resp.approved)
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

    #[cfg(feature = "derive")]
    pub use astrid_sdk_macros::wit_events;
}
