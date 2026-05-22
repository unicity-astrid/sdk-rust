//\! Interceptor handle registry — runtime-managed subscription handles
//\! that map to interceptor handler actions declared in `Capsule.toml`.

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
    /// Return a subscription handle for use with [`ipc::poll`] / [`ipc::recv`].
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
/// `handler(action, messages)` once with the typed [`ipc::PollResult`].
/// Bindings with no pending messages are skipped.
pub fn poll(
    bindings: &[InterceptorBinding],
    mut handler: impl FnMut(&str, &ipc::PollResult),
) -> Result<(), SysError> {
    for binding in bindings {
        let handle = binding.subscription_handle();
        let result = ipc::poll(&handle)?;
        if !result.messages.is_empty() {
            handler(&binding.action, &result);
        }
    }
    Ok(())
}
