//! Interceptor binding registry — metadata for the kernel-managed
//! interceptor subscriptions a capsule declared in `Capsule.toml`.
//!
//! Under the per-domain ABI, interceptor events are delivered to the
//! capsule through `astrid-hook-trigger` rather than a run-loop IPC
//! subscription. The host fn `get-interceptor-bindings` returns
//! metadata-only — capsules use it to enumerate the (action, topic)
//! pairs they're subscribed to for debugging, introspection, and
//! tooling. `handle-id` is the kernel-side registry handle and is not
//! convertible into an [`ipc::Subscription`]; it's surfaced for log
//! correlation only.

use super::*;

/// A single interceptor subscription binding.
#[derive(Debug, Clone)]
pub struct InterceptorBinding {
    /// Kernel-side subscription registry ID (opaque; for log
    /// correlation). NOT an [`ipc::Subscription`] handle.
    pub handle_id: u64,
    /// The interceptor action name from the manifest.
    pub action: String,
    /// The event topic this interceptor subscribes to.
    pub topic: String,
}

impl InterceptorBinding {
    /// Return the raw handle ID bytes (for lower-level interop / log
    /// correlation).
    #[must_use]
    pub fn handle_bytes(&self) -> Vec<u8> {
        self.handle_id.to_string().into_bytes()
    }
}

/// Query the runtime for auto-subscribed interceptor handles.
///
/// Returns an empty vec if this capsule has no auto-subscribed
/// interceptors (i.e. it does not have both `run()` and
/// `[[interceptor]]`). Each entry maps an action name to the IPC
/// topic the kernel routes through `astrid-hook-trigger`.
pub fn bindings() -> Result<Vec<InterceptorBinding>, SysError> {
    let handles = wit_ipc::get_interceptor_bindings().map_err(host_err)?;
    Ok(handles
        .into_iter()
        .map(|h| InterceptorBinding {
            handle_id: h.handle_id,
            action: h.action,
            topic: h.topic,
        })
        .collect())
}
