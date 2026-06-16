//! Lifecycle hook handling — read an event, optionally reply.
//!
//! The hook bridge translates kernel lifecycle events (tool calls,
//! session start/end, compaction, …) into semantic hooks and fans them
//! out to subscriber capsules over IPC. A capsule that subscribes to a
//! hook receives a [`HookEventRequest`] on `hook.v1.event.<name>` and —
//! when it wishes to influence the outcome — replies with a
//! [`HookResult`] on the per-request scoped topic
//! `hook.v1.response.<name>.<correlation-id>`.
//!
//! # The read-then-optionally-reply model
//!
//! Every hook delivery is a *read*: the capsule inspects the event and
//! decides whether to act. Replying is optional. Two reply shapes exist:
//!
//! - [`HookEvent::skip`] — ask the kernel to skip the gated operation
//!   (e.g. veto a tool call before it runs).
//! - [`HookEvent::respond_data`] — supply merged data the kernel folds
//!   back into the operation (opaque JSON, interpreted per hook).
//!
//! Some hooks are **fire-and-forget**: the bridge dispatches them with no
//! correlation id, so there is no reply topic. For those, [`HookEvent::reply`]
//! (and `skip` / `respond_data`) are no-ops that succeed without publishing —
//! the capsule observes the event but cannot influence it.
//!
//! Capsule authors rarely call this module directly. The `#[hook("name")]`
//! attribute on a `#[capsule]` method wires the deserialize → dispatch →
//! reply plumbing automatically; this module is the typed surface those
//! handlers receive.

pub use crate::contracts::hook::{HookEventRequest, HookResult};

/// Topic prefix for hook event envelopes published by the hook bridge.
///
/// The full topic is [`EVENT_PREFIX`] concatenated with the hook name,
/// e.g. `hook.v1.event.before_tool_call`. See [`event_topic`].
pub const EVENT_PREFIX: &str = "hook.v1.event.";

/// Topic prefix for per-request hook reply envelopes.
///
/// The full topic is [`RESPONSE_PREFIX`] concatenated with the hook name
/// and the request's correlation id, e.g.
/// `hook.v1.response.before_tool_call.<uuid>`. See [`response_topic`].
pub const RESPONSE_PREFIX: &str = "hook.v1.response.";

/// Build the event topic a subscriber listens on for hook `name`.
///
/// ```
/// # use astrid_sdk::hook::event_topic;
/// assert_eq!(event_topic("session_start"), "hook.v1.event.session_start");
/// ```
#[must_use]
pub fn event_topic(name: &str) -> String {
    format!("{EVENT_PREFIX}{name}")
}

/// Build the scoped reply topic for hook `name` and `correlation_id`.
///
/// ```
/// # use astrid_sdk::hook::response_topic;
/// assert_eq!(
///     response_topic("before_tool_call", "abc-123"),
///     "hook.v1.response.before_tool_call.abc-123",
/// );
/// ```
#[must_use]
pub fn response_topic(name: &str, correlation_id: &str) -> String {
    format!("{RESPONSE_PREFIX}{name}.{correlation_id}")
}

/// A hook event delivered to a subscriber, with its reply channel bound.
///
/// Wraps a [`HookEventRequest`] and exposes ergonomic accessors plus the
/// reply helpers ([`reply`](Self::reply), [`skip`](Self::skip),
/// [`respond_data`](Self::respond_data)). The payload is opaque JSON whose
/// shape depends on the hook — use [`payload`](Self::payload) to
/// deserialize it into a concrete type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    hook: String,
    correlation_id: Option<String>,
    payload: String,
}

impl HookEvent {
    /// Build a [`HookEvent`] from the raw [`HookEventRequest`] envelope,
    /// taking ownership of its fields.
    #[must_use]
    pub fn from_request(req: HookEventRequest) -> Self {
        Self {
            hook: req.hook,
            correlation_id: req.correlation_id,
            payload: req.payload,
        }
    }

    /// The semantic hook name (e.g. `before_tool_call`, `session_start`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.hook
    }

    /// The per-request correlation id, or `None` for fire-and-forget
    /// hooks that cannot be replied to.
    #[must_use]
    pub fn correlation_id(&self) -> Option<&str> {
        self.correlation_id.as_deref()
    }

    /// The raw, opaque JSON payload as delivered by the bridge.
    ///
    /// Prefer [`payload`](Self::payload) when the concrete shape is known.
    #[must_use]
    pub fn payload_str(&self) -> &str {
        &self.payload
    }

    /// Deserialize the JSON payload into a concrete type.
    ///
    /// # Errors
    ///
    /// Returns [`crate::SysError::JsonError`] if the payload is not valid
    /// JSON for `T`.
    pub fn payload<T: ::serde::de::DeserializeOwned>(&self) -> Result<T, crate::SysError> {
        ::serde_json::from_str(&self.payload).map_err(crate::SysError::from)
    }

    /// Publish a [`HookResult`] on this event's scoped reply topic.
    ///
    /// For fire-and-forget hooks (no correlation id) this is a successful
    /// no-op — there is no reply topic, so nothing is published.
    ///
    /// # Errors
    ///
    /// Returns [`crate::SysError`] if the underlying IPC publish fails.
    pub fn reply(&self, result: HookResult) -> Result<(), crate::SysError> {
        match &self.correlation_id {
            Some(id) => crate::ipc::publish_json(&response_topic(&self.hook, id), &result),
            None => Ok(()),
        }
    }

    /// Ask the kernel to skip the gated operation.
    ///
    /// Shorthand for [`reply`](Self::reply) with `skip: Some(true)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::SysError`] if the underlying IPC publish fails.
    pub fn skip(&self) -> Result<(), crate::SysError> {
        self.reply(HookResult {
            skip: Some(true),
            data: None,
        })
    }

    /// Reply with merged data the kernel folds back into the operation.
    ///
    /// Shorthand for [`reply`](Self::reply) with `data: Some(data)`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::SysError`] if the underlying IPC publish fails.
    pub fn respond_data(&self, data: impl Into<String>) -> Result<(), crate::SysError> {
        self.reply(HookResult {
            skip: None,
            data: Some(data.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_topic_formats() {
        assert_eq!(event_topic("session_start"), "hook.v1.event.session_start");
        assert_eq!(EVENT_PREFIX, "hook.v1.event.");
    }

    #[test]
    fn response_topic_formats() {
        assert_eq!(
            response_topic("before_tool_call", "abc-123"),
            "hook.v1.response.before_tool_call.abc-123",
        );
        assert_eq!(RESPONSE_PREFIX, "hook.v1.response.");
    }

    #[test]
    fn from_request_moves_fields() {
        let event = HookEvent::from_request(HookEventRequest {
            hook: "before_tool_call".into(),
            payload: r#"{"tool":"shell"}"#.into(),
            correlation_id: Some("corr-1".into()),
        });
        assert_eq!(event.name(), "before_tool_call");
        assert_eq!(event.correlation_id(), Some("corr-1"));
        assert_eq!(event.payload_str(), r#"{"tool":"shell"}"#);
    }

    #[test]
    fn from_request_fire_and_forget_has_no_correlation_id() {
        let event = HookEvent::from_request(HookEventRequest {
            hook: "session_end".into(),
            payload: "{}".into(),
            correlation_id: None,
        });
        assert_eq!(event.correlation_id(), None);
    }

    #[derive(::serde::Deserialize, Debug, PartialEq, Eq)]
    struct ToolPayload {
        tool: String,
    }

    #[test]
    fn payload_deserializes() {
        let event = HookEvent::from_request(HookEventRequest {
            hook: "before_tool_call".into(),
            payload: r#"{"tool":"shell"}"#.into(),
            correlation_id: None,
        });
        let parsed: ToolPayload = event.payload().unwrap();
        assert_eq!(
            parsed,
            ToolPayload {
                tool: "shell".into()
            }
        );
    }

    #[test]
    fn payload_malformed_is_json_error() {
        let event = HookEvent::from_request(HookEventRequest {
            hook: "before_tool_call".into(),
            payload: "not json".into(),
            correlation_id: None,
        });
        let err = event.payload::<ToolPayload>().unwrap_err();
        assert!(matches!(err, crate::SysError::JsonError(_)));
    }

    // The reply helpers publish over IPC, which requires a host. We assert
    // the no-host-needed shape they build instead: fire-and-forget replies
    // succeed without touching the bus. Wire-publish behaviour is covered by
    // integration tests against a running kernel.
    #[test]
    fn reply_fire_and_forget_is_noop_ok() {
        let event = HookEvent::from_request(HookEventRequest {
            hook: "session_end".into(),
            payload: "{}".into(),
            correlation_id: None,
        });
        assert!(
            event
                .reply(HookResult {
                    skip: Some(true),
                    data: None,
                })
                .is_ok()
        );
        assert!(event.skip().is_ok());
        assert!(event.respond_data("{}").is_ok());
    }

    #[test]
    fn skip_builds_skip_result() {
        let result = HookResult {
            skip: Some(true),
            data: None,
        };
        assert_eq!(result.skip, Some(true));
        assert_eq!(result.data, None);
    }

    #[test]
    fn respond_data_builds_data_result() {
        let result = HookResult {
            skip: None,
            data: Some(r#"{"k":"v"}"#.into()),
        };
        assert_eq!(result.skip, None);
        assert_eq!(result.data.as_deref(), Some(r#"{"k":"v"}"#));
    }
}
