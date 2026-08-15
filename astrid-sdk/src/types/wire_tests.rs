use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ContentPart, IpcMessage, IpcPayload, LlmResponse, Message, MessageContent, MessageRole,
    StopReason, StreamEvent, ToolCall, Usage,
};

#[test]
fn user_input_wire_shape_is_stable() {
    let payload = IpcPayload::UserInput {
        text: "hello".into(),
        session_id: "session-7".into(),
        context: Some(json!({"frontend": "cli"})),
    };

    assert_eq!(
        serde_json::to_value(payload).unwrap(),
        json!({
            "type": "user_input",
            "text": "hello",
            "session_id": "session-7",
            "context": {"frontend": "cli"}
        })
    );
}

#[test]
fn cli_proxy_frame_keeps_legacy_defaults() {
    let frame = json!({
        "topic": "agent.v1.response",
        "payload": {
            "type": "agent_response",
            "text": "done",
            "is_final": true,
            "session_id": "session-7"
        },
        "source_id": Uuid::nil()
    });

    let message: IpcMessage = serde_json::from_value(frame).unwrap();
    assert_eq!(message.topic, "agent.v1.response");
    assert!(message.signature.is_none());
    assert_eq!(message.seq, 0);
    assert!(message.principal.is_none());
    assert_eq!(message.timestamp.timestamp(), 0);
}

#[test]
fn llm_tool_message_wire_shape_is_stable() {
    let message = Message::assistant_with_tools(vec![
        ToolCall::new("call-1", "filesystem:read_file")
            .with_arguments(json!({"path": "/tmp/example"})),
    ]);

    assert_eq!(
        serde_json::to_value(message).unwrap(),
        json!({
            "role": "assistant",
            "content": [{
                "id": "call-1",
                "name": "filesystem:read_file",
                "arguments": {"path": "/tmp/example"}
            }]
        })
    );
}

#[test]
fn llm_stream_and_response_wire_shapes_are_stable() {
    assert_eq!(
        serde_json::to_value(StreamEvent::ToolCallStart {
            id: "call-1".into(),
            name: "search".into(),
        })
        .unwrap(),
        json!({"ToolCallStart": {"id": "call-1", "name": "search"}})
    );

    let response = LlmResponse {
        message: Message {
            role: MessageRole::Assistant,
            content: MessageContent::MultiPart(vec![ContentPart::Text {
                text: "hello".into(),
            }]),
        },
        has_tool_calls: false,
        stop_reason: StopReason::EndTurn,
        usage: Usage {
            input_tokens: 3,
            output_tokens: 2,
        },
    };
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["stop_reason"], Value::String("EndTurn".into()));
    assert_eq!(
        value["usage"],
        json!({"input_tokens": 3, "output_tokens": 2})
    );
    assert_eq!(
        value["message"]["content"],
        json!([{"type": "text", "text": "hello"}])
    );
}
