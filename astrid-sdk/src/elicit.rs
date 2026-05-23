//! Interactive user input during install/upgrade lifecycle.
//!
//! All elicit operations go through the typed
//! `astrid:elicit/host@1.0.0` package. The host returns a typed
//! `ElicitResponse` variant (`Value(String)` / `Values(Vec<String>)` /
//! `SecretStored`) rather than a JSON-string blob, so the wrappers
//! below are no-string-parsing.

use super::*;

/// Validates that the elicit key is non-empty and not whitespace-only.
fn validate_key(key: &str) -> Result<(), SysError> {
    if key.trim().is_empty() {
        return Err(SysError::ApiError("elicit key must not be empty".into()));
    }
    Ok(())
}

fn build_request(
    kind: wit_elicit::ElicitType,
    key: &str,
    description: &str,
    options: Option<Vec<String>>,
    default_value: Option<String>,
) -> wit_elicit::ElicitRequest {
    wit_elicit::ElicitRequest {
        kind,
        key: key.to_string(),
        description: description.to_string(),
        options,
        default_value,
    }
}

/// Store a secret via the kernel's `SecretStore`. The capsule **never**
/// receives the value. Returns `Ok(())` confirming the user provided it.
pub fn secret(key: &str, description: &str) -> Result<(), SysError> {
    validate_key(key)?;
    let req = build_request(wit_elicit::ElicitType::Secret, key, description, None, None);
    match wit_elicit::elicit(&req).map_err(host_err)? {
        wit_elicit::ElicitResponse::SecretStored => Ok(()),
        other => Err(SysError::ApiError(format!(
            "host returned unexpected response for secret elicit: {other:?}"
        ))),
    }
}

/// Check if a secret has been configured (without reading it).
pub fn has_secret(key: &str) -> Result<bool, SysError> {
    validate_key(key)?;
    wit_elicit::has_secret(key).map_err(host_err)
}

/// Shared implementation for text elicitation with optional default.
fn elicit_text(key: &str, description: &str, default: Option<&str>) -> Result<String, SysError> {
    validate_key(key)?;
    let req = build_request(
        wit_elicit::ElicitType::Text,
        key,
        description,
        None,
        default.map(str::to_string),
    );
    match wit_elicit::elicit(&req).map_err(host_err)? {
        wit_elicit::ElicitResponse::Value(v) => Ok(v),
        other => Err(SysError::ApiError(format!(
            "host returned unexpected response for text elicit: {other:?}"
        ))),
    }
}

/// Prompt for a text value. Blocks until the user responds.
/// Use [`secret()`] for sensitive data - this returns the value to the capsule.
pub fn text(key: &str, description: &str) -> Result<String, SysError> {
    elicit_text(key, description, None)
}

/// Prompt with a default value pre-filled.
pub fn text_with_default(key: &str, description: &str, default: &str) -> Result<String, SysError> {
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
    let owned: Vec<String> = options.iter().map(|s| (*s).to_string()).collect();
    let req = build_request(
        wit_elicit::ElicitType::Select,
        key,
        description,
        Some(owned),
        None,
    );
    let value = match wit_elicit::elicit(&req).map_err(host_err)? {
        wit_elicit::ElicitResponse::Value(v) => v,
        other => {
            return Err(SysError::ApiError(format!(
                "host returned unexpected response for select elicit: {other:?}"
            )));
        }
    };
    if !options.iter().any(|o| *o == value) {
        let truncated: String = value.chars().take(64).collect();
        return Err(SysError::ApiError(format!(
            "host returned value '{truncated}' not in provided options",
        )));
    }
    Ok(value)
}

/// Prompt for multiple text values (array input).
pub fn array(key: &str, description: &str) -> Result<Vec<String>, SysError> {
    validate_key(key)?;
    let req = build_request(wit_elicit::ElicitType::Array, key, description, None, None);
    match wit_elicit::elicit(&req).map_err(host_err)? {
        wit_elicit::ElicitResponse::Values(v) => Ok(v),
        other => Err(SysError::ApiError(format!(
            "host returned unexpected response for array elicit: {other:?}"
        ))),
    }
}
