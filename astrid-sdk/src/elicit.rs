//\! Interactive user input during install/upgrade lifecycle.

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
