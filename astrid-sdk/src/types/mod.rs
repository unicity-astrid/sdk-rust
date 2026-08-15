//! Capsule-facing Rust representations of Astrid wire data.
//!
//! These types belong to the SDK. The canonical compatibility boundary is the
//! serialized wire shape (and the WIT contracts where applicable), not a
//! dependency on Astrid core's internal crate graph.

use serde::{Deserialize, Serialize};

pub mod ipc;
pub mod llm;

pub use ipc::{IpcMessage, IpcPayload, OnboardingField, OnboardingFieldType, SelectionOption};
pub use llm::{
    ContentPart, LlmResponse, LlmToolDefinition, Message, MessageContent, MessageRole, StopReason,
    StreamEvent, ToolCall, ToolCallResult, Usage,
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

#[cfg(test)]
mod wire_tests;
