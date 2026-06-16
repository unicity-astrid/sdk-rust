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

    // Sub-modules (re-exported for `astrid_sdk::types::ipc::*` access).
    //
    // `kernel`/`kernel_api` is intentionally NOT re-exported here —
    // those CLI ↔ daemon RPC types now live in `astrid_core::kernel_api`
    // (post-PR#752 decoupling) and don't belong in capsule space.
    // Capsules use `ipc` and `llm` types only.
    pub use astrid_types::ipc;
    pub use astrid_types::llm;

    // IPC types
    pub use astrid_types::ipc::{
        IpcMessage, IpcPayload, OnboardingField, OnboardingFieldType, SelectionOption,
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

/// Core error type for SDK operations.
///
/// Per-domain WIT host fns return their own typed `ErrorCode` enum
/// (`astrid:fs/host.error-code`, `astrid:ipc/host.error-code`, etc.).
/// At the SDK boundary every such typed error is converted to
/// [`SysError::HostError`] via [`host_err`] (a `Debug` formatting),
/// keeping the existing capsule-author-facing surface stable.
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

/// Convert any per-domain `ErrorCode` (or other `Debug` host failure)
/// into a [`SysError::HostError`] via `Debug` formatting.
///
/// The migration guide instructs the SDK to surface typed kernel errors
/// uniformly through `SysError::HostError(String)`. This keeps the
/// capsule-author API stable across the WIT split while still carrying
/// the typed variant name (e.g. `"CapabilityDenied"`,
/// `"Unknown(\"port pending\")"`) in the error string for log parity.
pub(crate) fn host_err<E: core::fmt::Debug>(e: E) -> SysError {
    SysError::HostError(format!("{e:?}"))
}

/// Install a panic hook that routes Rust panics through
/// [`crate::log::error`] before the wasm process traps.
///
/// On `wasm32-unknown-unknown` the default panic strategy is `abort`
/// — without a hook, the panic message is lost and the kernel only
/// sees an opaque wasm trap with a numbered backtrace. Capsule logs
/// then show no `panic at src/lib.rs:42` style line, making
/// triage from the per-capsule log file impossible.
///
/// Called automatically by the generated `#[capsule]` entry points
/// (every `Guest` trait method calls this on first entry). The
/// `Once` guard makes repeated calls cheap and idempotent.
///
/// Not part of the documented capsule-author surface — invocation is
/// generated by `astrid-sdk-macros`. Marked `#[doc(hidden)]` for that
/// reason.
#[doc(hidden)]
pub fn install_panic_handler() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let payload = info
                .payload()
                .downcast_ref::<&'static str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());
            log::error(format!("capsule panic at {location}: {payload}"));
        }));
    });
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

    /// Uplink profile — how the kernel routes inbound messages.
    ///
    /// Mirrors the `astrid:uplink/host.uplink-profile` enum. The
    /// variants name the canonical interaction patterns the kernel
    /// recognises.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Profile {
        /// Conversational chat (Telegram, Discord DM, Slack DM).
        Chat,
        /// Long-lived interactive session (CLI TTY).
        Interactive,
        /// One-way notification sink.
        Notify,
        /// Bidirectional bridge to another runtime.
        Bridge,
    }

    impl Profile {
        fn to_wit(self) -> wit_uplink::UplinkProfile {
            match self {
                Self::Chat => wit_uplink::UplinkProfile::Chat,
                Self::Interactive => wit_uplink::UplinkProfile::Interactive,
                Self::Notify => wit_uplink::UplinkProfile::Notify,
                Self::Bridge => wit_uplink::UplinkProfile::Bridge,
            }
        }

        /// Parse a profile from one of the canonical lowercase names.
        ///
        /// Accepts `"chat"`, `"interactive"`, `"notify"`, `"bridge"`.
        /// Returns [`SysError::ApiError`] for any other value.
        ///
        /// Named `parse` rather than `from_str` to avoid the
        /// `std::str::FromStr` trait-method shadowing trap (the SDK's
        /// error type isn't `FromStr::Err`'s expected shape).
        pub fn parse(s: &str) -> Result<Self, SysError> {
            match s {
                "chat" => Ok(Self::Chat),
                "interactive" => Ok(Self::Interactive),
                "notify" => Ok(Self::Notify),
                "bridge" => Ok(Self::Bridge),
                other => Err(SysError::ApiError(format!(
                    "unknown uplink profile: {other}"
                ))),
            }
        }
    }

    /// Register a new uplink connection. Returns a typed [`UplinkId`].
    ///
    /// `profile` is one of `"chat"`, `"interactive"`, `"notify"`, `"bridge"`.
    pub fn register(name: &str, platform: &str, profile: &str) -> Result<UplinkId, SysError> {
        let parsed = Profile::parse(profile)?;
        let id = wit_uplink::uplink_register(name, platform, parsed.to_wit()).map_err(host_err)?;
        Ok(UplinkId(id))
    }

    /// Register a new uplink connection with a typed [`Profile`].
    pub fn register_profile(
        name: &str,
        platform: &str,
        profile: Profile,
    ) -> Result<UplinkId, SysError> {
        let id = wit_uplink::uplink_register(name, platform, profile.to_wit()).map_err(host_err)?;
        Ok(UplinkId(id))
    }

    /// Send a message to a user via an uplink.
    ///
    /// Returns `true` if sent, `false` if the message was dropped
    /// (no active session for the target principal).
    pub fn send(
        uplink_id: &UplinkId,
        platform_user_id: &str,
        content: &str,
    ) -> Result<bool, SysError> {
        wit_uplink::uplink_send(&uplink_id.0, platform_user_id, content).map_err(host_err)
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
    ///
    /// Returns an empty `Vec` if the key is not set in the manifest
    /// (matching the pre-migration shape so capsule code that treats
    /// "missing key" and "empty value" the same continues to compile).
    pub fn var_bytes(key: &str) -> Result<Vec<u8>, SysError> {
        let value = wit_sys::get_config(key).map_err(host_err)?;
        Ok(value.unwrap_or_default().into_bytes())
    }

    /// Read a config value as a UTF-8 string. Like `std::env::var`.
    ///
    /// Returns an empty string if the key is not set in the manifest.
    /// To distinguish "missing" from "empty", use [`var_opt`].
    pub fn var(key: &str) -> Result<String, SysError> {
        let value = wit_sys::get_config(key).map_err(host_err)?;
        Ok(value.unwrap_or_default())
    }

    /// Read a config value, returning `None` if the key is not set.
    ///
    /// The empty string is a valid value distinct from `None`.
    pub fn var_opt(key: &str) -> Result<Option<String>, SysError> {
        wit_sys::get_config(key).map_err(host_err)
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
    /// system clock. Returns `Result` for API symmetry; the underlying
    /// host fn is infallible.
    pub fn now() -> Result<std::time::SystemTime, SysError> {
        let ms = wit_sys::clock_ms();
        Ok(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms))
    }

    /// Returns a monotonic clock reading.
    ///
    /// Suitable for measuring elapsed time within an invocation. Does
    /// not jump with NTP adjustments. The absolute value is meaningless
    /// across capsule reloads — only differences are.
    pub fn monotonic() -> std::time::Duration {
        std::time::Duration::from_nanos(wit_sys::clock_monotonic_ns())
    }

    /// Block the calling task for the given duration. Capped server-side
    /// at 60 seconds per call; loop on shorter sleeps for longer waits.
    pub fn sleep(duration: std::time::Duration) -> Result<(), SysError> {
        let ns = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        wit_sys::sleep_ns(ns).map_err(host_err)
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
        wit_sys::log(wit_sys::LogLevel::Trace, &format!("{message}"));
    }

    /// Log at DEBUG level.
    pub fn debug(message: impl Display) {
        wit_sys::log(wit_sys::LogLevel::Debug, &format!("{message}"));
    }

    /// Log at INFO level.
    pub fn info(message: impl Display) {
        wit_sys::log(wit_sys::LogLevel::Info, &format!("{message}"));
    }

    /// Log at WARN level.
    pub fn warn(message: impl Display) {
        wit_sys::log(wit_sys::LogLevel::Warn, &format!("{message}"));
    }

    /// Log at ERROR level.
    pub fn error(message: impl Display) {
        wit_sys::log(wit_sys::LogLevel::Error, &format!("{message}"));
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
        let ctx = wit_sys::get_caller().map_err(host_err)?;
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
        let path = crate::env::var_opt(crate::env::CONFIG_SOCKET_PATH)?.ok_or_else(|| {
            SysError::ApiError("ASTRID_SOCKET_PATH config key is not set".to_string())
        })?;
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

    /// Fill the requested length with cryptographically secure random
    /// bytes from the host's OS-level CSPRNG.
    ///
    /// Capped server-side at 4096 bytes per call; loop for bulk
    /// entropy. Suitable for cryptographic key material.
    pub fn random_bytes(length: usize) -> Result<Vec<u8>, SysError> {
        let len = u64::try_from(length).unwrap_or(u64::MAX);
        wit_sys::random_bytes(len).map_err(host_err)
    }
}

/// Capability introspection.
///
/// [`check`] asks whether a capsule (self or any other, by IPC session UUID)
/// holds a specific manifest capability — used by the prompt builder to
/// enforce `allow_prompt_injection` gating. [`enumerate`] is the list dual for
/// the calling capsule's own set: the names for which a self-`check` returns
/// `true`. Capability posture is structural metadata, not a secret
/// (enforce-don't-conceal), so both are ungated.
pub mod capabilities {
    use super::*;

    /// Check whether a capsule has a specific capability.
    ///
    /// Returns `true` if the capsule identified by `source_uuid` has the
    /// given `capability` declared in its manifest. Returns `false` for
    /// unknown UUIDs, unknown capabilities (fail-closed).
    pub fn check(source_uuid: &str, capability: &str) -> Result<bool, SysError> {
        let request = wit_sys::CapabilityCheckRequest {
            source_uuid: source_uuid.to_string(),
            capability: capability.to_string(),
        };
        let response = wit_sys::check_capsule_capability(&request).map_err(host_err)?;
        Ok(response.allowed)
    }

    /// Enumerate the calling capsule's own held capability names.
    ///
    /// Returns the capability categories declared in this capsule's
    /// `[capabilities]` manifest block (`host_process`, `net_connect`,
    /// `fs_read`, …) — the names, not the scoped arguments within them
    /// (allowlists, `host:port`, paths). This is exactly the set of names for
    /// which [`check`] against this capsule's own UUID returns `true`.
    ///
    /// Argument-free (the kernel already knows the caller) and infallible: the
    /// kernel always knows the caller's own registered set, so there is no
    /// error path — an empty list is the valid "no capabilities" answer. Lets
    /// a reusable capsule ground its behaviour in what it can actually do
    /// instead of hard-coding it, avoiding code-vs-manifest drift.
    pub fn enumerate() -> Vec<String> {
        wit_sys::enumerate_capabilities()
    }
}

pub mod net;
pub mod process;

/// Lifecycle hook handling — read kernel lifecycle events, optionally reply.
///
/// The hook bridge fans semantic hooks (tool calls, session lifecycle,
/// compaction, …) out to subscriber capsules as [`hook::HookEvent`]s. A
/// subscriber inspects the event and may reply with a [`hook::HookResult`]
/// to skip the gated operation or merge data back into it. The
/// `#[hook("name")]` attribute on a `#[capsule]` method wires this up.
///
/// Re-exports the canonical [`HookEventRequest`](hook::HookEventRequest) and
/// [`HookResult`](hook::HookResult) from [`contracts`], so it is only
/// available with the `derive` feature (enabled by default).
#[cfg(feature = "derive")]
pub mod hook;

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

    /// The decision returned by the host (or by an existing allowance
    /// pattern) for an [`request`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Decision {
        /// Denied — capsule must not proceed.
        Denied,
        /// Approved once.
        Approved,
        /// Approved for the current session.
        ApprovedSession,
        /// Approved permanently (stored in the AllowanceStore).
        ApprovedAlways,
        /// Auto-approved via an existing allowance pattern.
        Allowance,
    }

    impl Decision {
        /// Whether this decision permits the action.
        #[must_use]
        pub fn is_approved(self) -> bool {
            !matches!(self, Self::Denied)
        }

        fn from_wit(d: wit_approval::ApprovalDecision) -> Self {
            match d {
                wit_approval::ApprovalDecision::Denied => Self::Denied,
                wit_approval::ApprovalDecision::Approved => Self::Approved,
                wit_approval::ApprovalDecision::ApprovedSession => Self::ApprovedSession,
                wit_approval::ApprovalDecision::ApprovedAlways => Self::ApprovedAlways,
                wit_approval::ApprovalDecision::Allowance => Self::Allowance,
            }
        }
    }

    /// Request human approval for a sensitive action.
    ///
    /// Blocks the capsule until the frontend user responds or the request
    /// times out. If an existing allowance matches, returns immediately
    /// without prompting.
    ///
    /// Returns `true` if approved (any approval variant), `false` if denied.
    /// For the specific decision class (one-shot vs session vs always vs
    /// allowance-hit), use [`request_decision`].
    ///
    /// # Example
    /// ```ignore
    /// if approval::request("git push", "git push origin main")? {
    ///     // proceed with the push
    /// }
    /// ```
    pub fn request(action: &str, resource: &str) -> Result<bool, SysError> {
        Ok(request_decision(action, resource)?.is_approved())
    }

    /// Request human approval and return the specific [`Decision`].
    pub fn request_decision(action: &str, resource: &str) -> Result<Decision, SysError> {
        let req = wit_approval::ApprovalRequest {
            action: action.to_string(),
            target_resource: resource.to_string(),
        };
        let resp = wit_approval::request_approval(&req).map_err(host_err)?;
        Ok(Decision::from_wit(resp.decision))
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

    // Hook handling re-exports the `derive`-gated `contracts` types, so the
    // module is only available with the `derive` feature.
    #[cfg(feature = "derive")]
    pub use crate::hook;

    #[cfg(feature = "derive")]
    pub use astrid_sdk_macros::capsule;

    #[cfg(feature = "derive")]
    pub use astrid_sdk_macros::wit_events;
}
