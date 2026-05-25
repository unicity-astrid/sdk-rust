//! Event bus messaging — topic-based pub/sub plus the
//! `request_response` helper that wraps the capsule-conventional
//! correlation-id handshake.
//!
//! The bus carries UTF-8 string payloads (WIT `string` type); structured
//! data goes through `serde_json` via the `*_json` helpers.
//!
//! Subscriptions are RAII handles. Drop releases the kernel-side
//! resource — capsules do not call `unsubscribe` explicitly.

use super::*;

/// An active subscription to an IPC topic.
///
/// Owns the kernel-side [`astrid_sys::astrid::ipc::host::Subscription`]
/// resource. Dropping this value tears the subscription down — the
/// component-model runtime invokes the resource's destructor — so
/// idiomatic Rust scoping handles cleanup without manual
/// `unsubscribe` calls.
///
/// Wraps the raw resource in a typed Rust struct so capsule code calls
/// `sub.poll()` / `sub.recv(timeout_ms)` instead of bare host fns.
#[derive(Debug)]
pub struct Subscription {
    inner: wit_ipc::Subscription,
}

impl Subscription {
    /// Non-blocking poll for messages queued since the last
    /// `poll` / `recv` on this subscription.
    pub fn poll(&self) -> Result<PollResult, SysError> {
        let envelope = self.inner.poll().map_err(host_err)?;
        Ok(envelope_to_poll_result(envelope))
    }

    /// Block until a message arrives on this subscription, or
    /// `timeout_ms` elapses.
    ///
    /// Max timeout: 60_000 ms (capped by the host). Returns
    /// [`SysError::HostError`] containing `"Timeout"` if no message
    /// arrives within the deadline.
    pub fn recv(&self, timeout_ms: u64) -> Result<PollResult, SysError> {
        let envelope = self.inner.recv(timeout_ms).map_err(host_err)?;
        Ok(envelope_to_poll_result(envelope))
    }
}

/// Publish a UTF-8 string payload to an IPC topic.
///
/// The IPC bus carries UTF-8 strings (WIT `string` type). For structured
/// data, use [`publish_json`] which handles serialization.
pub fn publish(topic: &str, payload: &str) -> Result<(), SysError> {
    wit_ipc::publish(topic, payload).map_err(host_err)
}

/// Publish a JSON-serialized value to an IPC topic.
pub fn publish_json<T: Serialize>(topic: &str, payload: &T) -> Result<(), SysError> {
    let json = serde_json::to_string(payload)?;
    publish(topic, &json)
}

/// Publish a UTF-8 string payload on behalf of a specific principal.
///
/// Like [`publish`] but stamps the outgoing envelope with the supplied
/// `principal` instead of this capsule's. Reserved for uplinks
/// (`uplink = true` in `Capsule.toml [capabilities]`) — the host
/// returns an error for any other caller. The trust model is "trust
/// the uplink": the kernel does not verify that the uplink actually
/// authenticated the claimed principal. Subscribers see the principal
/// as `claimed(...)`, not `verified(...)`. See issue #658.
pub fn publish_as(topic: &str, payload: &str, principal: &str) -> Result<(), SysError> {
    wit_ipc::publish_as(topic, payload, principal).map_err(host_err)
}

/// Publish a JSON-serialized value on behalf of a specific principal.
/// See [`publish_as`].
pub fn publish_json_as<T: Serialize>(
    topic: &str,
    payload: &T,
    principal: &str,
) -> Result<(), SysError> {
    let json = serde_json::to_string(payload)?;
    publish_as(topic, &json, principal)
}

/// Subscribe to an IPC topic.
///
/// Returns a [`Subscription`] handle whose `Drop` releases the
/// kernel-side resource. Per-capsule cap: 128 active subscriptions.
/// Supports exact matches and trailing-suffix wildcards
/// (`foo.bar.*`); mid-segment wildcards are rejected by the host.
pub fn subscribe(topic_pattern: &str) -> Result<Subscription, SysError> {
    let inner = wit_ipc::subscribe(topic_pattern).map_err(host_err)?;
    Ok(Subscription { inner })
}

/// Provenance + trust attribution for an IPC message's principal.
///
/// Mirrors `astrid:ipc/host.principal-attribution`. Capsules processing
/// sensitive actions MUST inspect this — a `Claimed` principal is
/// uplink-asserted and not kernel-verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrincipalAttribution {
    /// Principal kernel-verified from the publishing capsule's invocation
    /// context. Trusted for capability checks.
    Verified(String),
    /// Principal claimed by an uplink capsule via `publish_as`. Uplink-
    /// asserted, NOT kernel-verified.
    Claimed(String),
    /// System / kernel-originated event with no attributable principal.
    System,
}

impl PrincipalAttribution {
    fn from_wit(p: wit_ipc::PrincipalAttribution) -> Self {
        match p {
            wit_ipc::PrincipalAttribution::Verified(s) => Self::Verified(s),
            wit_ipc::PrincipalAttribution::Claimed(s) => Self::Claimed(s),
            wit_ipc::PrincipalAttribution::System => Self::System,
        }
    }

    /// Returns the principal string if this is a `Verified` attribution,
    /// otherwise `None`. Use this for capability-gated actions.
    #[must_use]
    pub fn verified(&self) -> Option<&str> {
        match self {
            Self::Verified(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// A message received from the IPC bus.
#[derive(Debug, Clone)]
pub struct Message {
    /// The topic this message was published on.
    pub topic: String,
    /// The message payload (JSON string).
    pub payload: String,
    /// UUID of the capsule that published this message.
    pub source_id: String,
    /// Principal attribution — see [`PrincipalAttribution`].
    pub principal: PrincipalAttribution,
}

/// Result of polling or receiving from an IPC subscription.
#[derive(Debug, Clone)]
pub struct PollResult {
    /// Messages received since the last poll/recv.
    pub messages: Vec<Message>,
    /// Number of messages dropped due to buffer overflow.
    pub dropped: u64,
    /// Cumulative lag (messages missed due to slow consumption).
    pub lagged: u64,
}

/// Send a request on `request_topic` and await a single response on a
/// scoped reply topic.
///
/// The helper:
/// 1. Generates a v4 correlation ID.
/// 2. Subscribes to `{response_namespace}.{correlation_id}` *before*
///    publishing (so the response can never be missed in the race).
/// 3. Injects the correlation ID into the request payload as a top-level
///    `"correlation_id"` field.
/// 4. Publishes the request.
/// 5. Blocks up to `timeout_ms` for the response.
/// 6. Unsubscribes (always — on success or error — via Drop on the
///    [`Subscription`] handle).
/// 7. Deserializes the response payload into `Resp`.
///
/// `Req` must serialize to a JSON object — primitives, arrays, etc. are
/// rejected at runtime with [`SysError::ApiError`] because there is
/// nowhere to put the correlation ID.
///
/// `response_namespace` should be the dotted topic prefix the responder
/// publishes to, *without* the trailing correlation id segment. For
/// example, if the responder publishes to
/// `registry.v1.response.set_active_model.<corr_id>`, pass
/// `"registry.v1.response.set_active_model"`.
///
/// `timeout_ms` is capped at 60,000 ms by the host. A timeout returns
/// [`SysError::ApiError`] with a `request_response: no reply within …`
/// message.
///
/// # Errors
///
/// - [`SysError::JsonError`] if `request` does not serialize to JSON or
///   `Resp` does not deserialize from the response payload.
/// - [`SysError::HostError`] if any underlying IPC host call fails.
/// - [`SysError::ApiError`] if the request payload is not a JSON object,
///   or no reply arrives within the timeout.
pub fn request_response<Req, Resp>(
    request_topic: &str,
    response_namespace: &str,
    request: &Req,
    timeout_ms: u64,
) -> Result<Resp, SysError>
where
    Req: Serialize,
    Resp: serde::de::DeserializeOwned,
{
    // Validate input before touching the host so bad calls never allocate a
    // subscription handle.
    let correlation_id = uuid::Uuid::new_v4().to_string();
    let mut value = serde_json::to_value(request)?;
    let serde_json::Value::Object(map) = &mut value else {
        return Err(SysError::ApiError(
            "request_response: request payload must serialize to a JSON object so the \
             correlation_id can be injected"
                .to_string(),
        ));
    };
    map.insert(
        "correlation_id".to_string(),
        serde_json::Value::String(correlation_id.clone()),
    );
    let payload = serde_json::to_string(&value)?;

    let reply_topic = format!("{response_namespace}.{correlation_id}");

    // Subscribe BEFORE publishing — the responder may publish the reply
    // before we'd otherwise have a chance to subscribe. The Drop on
    // `sub` tears the subscription down on every return path.
    let sub = subscribe(&reply_topic)?;

    publish(request_topic, &payload)?;

    let poll = sub.recv(timeout_ms)?;
    let msg = poll.messages.into_iter().next().ok_or_else(|| {
        SysError::ApiError(format!(
            "request_response: no reply on '{reply_topic}' within {timeout_ms}ms"
        ))
    })?;

    let resp: Resp = serde_json::from_str(&msg.payload)?;
    Ok(resp)
}

fn envelope_to_poll_result(envelope: wit_ipc::IpcEnvelope) -> PollResult {
    PollResult {
        messages: envelope
            .messages
            .into_iter()
            .map(|m| Message {
                topic: m.topic,
                payload: m.payload,
                source_id: m.source_id,
                principal: PrincipalAttribution::from_wit(m.principal),
            })
            .collect(),
        dropped: envelope.dropped,
        lagged: envelope.lagged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct NotAnObject(String);

    #[test]
    fn request_response_rejects_non_object_payload_before_host_call() {
        // Validation runs before any host call, so this path can be
        // exercised on the native test target where the WIT host imports
        // are unimplemented. A non-object payload (here, a tuple struct
        // serializing as a string) must produce `ApiError` with the
        // expected message — and NOT a panic from attempting a host call.
        let err = request_response::<NotAnObject, serde_json::Value>(
            "req.topic",
            "resp.topic",
            &NotAnObject("hello".to_string()),
            100,
        )
        .unwrap_err();
        match err {
            SysError::ApiError(msg) => assert!(
                msg.contains("must serialize to a JSON object"),
                "unexpected ApiError message: {msg}"
            ),
            other => panic!("expected ApiError, got {other:?}"),
        }
    }
}
