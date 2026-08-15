use astrid_sdk::types::{
    IpcPayload, Message,
    ipc::{IpcPayload as ModuleIpcPayload, OnboardingFieldType},
    llm::{Message as ModuleMessage, MessageRole},
};

#[test]
fn legacy_type_paths_remain_available() {
    let direct = Message::user("hello");
    let module: ModuleMessage = direct;
    assert_eq!(module.role, MessageRole::User);

    let direct = IpcPayload::Connect;
    let module: ModuleIpcPayload = direct;
    assert!(matches!(module, ModuleIpcPayload::Connect));
    assert_eq!(OnboardingFieldType::Text, OnboardingFieldType::Text);
}
