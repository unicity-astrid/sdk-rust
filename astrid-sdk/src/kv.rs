//\! Per-capsule, per-principal key-value storage.

use super::*;

pub fn get_bytes(key: &str) -> Result<Vec<u8>, SysError> {
    let key_str = key;
    let result = wit_kv::kv_get(key_str).map_err(SysError::HostError)?;
    Ok(result.unwrap_or_default())
}

pub fn set_bytes(key: &str, value: &[u8]) -> Result<(), SysError> {
    let key_str = key;
    wit_kv::kv_set(key_str, value).map_err(SysError::HostError)
}

pub fn get_json<T: DeserializeOwned>(key: &str) -> Result<T, SysError> {
    let bytes = get_bytes(key)?;
    let parsed = serde_json::from_slice(&bytes)?;
    Ok(parsed)
}

pub fn set_json<T: Serialize>(key: &str, value: &T) -> Result<(), SysError> {
    let bytes = serde_json::to_vec(value)?;
    set_bytes(key, &bytes)
}

/// Delete a key from the KV store.
///
/// This is idempotent: deleting a non-existent key succeeds silently.
/// The underlying store returns whether the key existed, but that
/// information is not surfaced through the WASM host boundary.
pub fn delete(key: &str) -> Result<(), SysError> {
    let key_str = key;
    wit_kv::kv_delete(key_str).map_err(SysError::HostError)
}

/// List all keys matching a prefix.
///
/// Returns an empty vec if no keys match. The prefix is matched
/// against key names within the capsule's scoped namespace.
pub fn list_keys(prefix: &str) -> Result<Vec<String>, SysError> {
    let prefix_str = prefix;
    wit_kv::kv_list_keys(prefix_str).map_err(SysError::HostError)
}

/// Delete all keys matching a prefix.
///
/// Returns the number of keys deleted. The prefix is matched
/// against key names within the capsule's scoped namespace.
pub fn clear_prefix(prefix: &str) -> Result<u64, SysError> {
    let prefix_str = prefix;
    wit_kv::kv_clear_prefix(prefix_str).map_err(SysError::HostError)
}

pub fn get_borsh<T: BorshDeserialize>(key: &str) -> Result<T, SysError> {
    let bytes = get_bytes(key)?;
    let parsed = borsh::from_slice(&bytes)?;
    Ok(parsed)
}

pub fn set_borsh<T: BorshSerialize>(key: &str, value: &T) -> Result<(), SysError> {
    let bytes = borsh::to_vec(value)?;
    set_bytes(key, &bytes)
}

// ---- Versioned KV helpers ----

/// Internal envelope for versioned KV data.
///
/// Wire format: `{"__sv": <version>, "data": <payload>}`.
/// The `__sv` prefix is deliberately ugly to avoid collision with
/// user struct fields.
#[derive(Serialize, Deserialize)]
struct VersionedEnvelope<T> {
    #[serde(rename = "__sv")]
    schema_version: u32,
    data: T,
}

/// Result of reading versioned data from KV.
#[derive(Debug)]
pub enum Versioned<T> {
    /// Data is at the expected schema version.
    Current(T),
    /// Data is at an older version and needs migration.
    NeedsMigration {
        /// Raw JSON value of the `data` field.
        raw: serde_json::Value,
        /// The schema version that was stored.
        stored_version: u32,
    },
    /// Key exists but data has no version envelope (pre-versioning legacy data).
    Unversioned(serde_json::Value),
    /// Key does not exist in KV.
    NotFound,
}

/// Write versioned data to KV, wrapped in a schema-version envelope.
///
/// The stored JSON looks like `{"__sv": 1, "data": { ... }}`.
/// Use [`get_versioned`] or [`get_versioned_or_migrate`] to read it back.
pub fn set_versioned<T: Serialize>(key: &str, value: &T, version: u32) -> Result<(), SysError> {
    let envelope = VersionedEnvelope {
        schema_version: version,
        data: value,
    };
    set_json(key, &envelope)
}

/// Read versioned data from KV.
///
/// Returns [`Versioned::Current`] if the stored version matches
/// `current_version`. Returns [`Versioned::NeedsMigration`] for older
/// versions. Returns an error for versions newer than `current_version`
/// (fail secure - don't silently interpret data from a schema you don't
/// understand).
///
/// Data written by plain [`set_json`] (no envelope) returns
/// [`Versioned::Unversioned`].
pub fn get_versioned<T: DeserializeOwned>(
    key: &str,
    current_version: u32,
) -> Result<Versioned<T>, SysError> {
    let bytes = get_bytes(key)?;
    parse_versioned(&bytes, current_version)
}

/// Core parsing logic for versioned KV data, separated from FFI for
/// testability. Operates on raw bytes as returned by `get_bytes`.
fn parse_versioned<T: DeserializeOwned>(
    bytes: &[u8],
    current_version: u32,
) -> Result<Versioned<T>, SysError> {
    // The host function `kv_get` returns an empty slice when the
    // key is absent. A present key written via set_json/set_versioned
    // always has at least the JSON envelope bytes, so empty = not found.
    if bytes.is_empty() {
        return Ok(Versioned::NotFound);
    }

    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;

    // Detect envelope by checking for __sv (u64) + data fields.
    // If __sv is present but malformed (not a number, or missing data),
    // return an error rather than silently treating as unversioned.
    let sv_field = value.get("__sv");
    let has_sv = sv_field.is_some();
    let envelope_version = sv_field.and_then(|v| v.as_u64());
    let has_data = value.get("data").is_some();

    match (has_sv, envelope_version, has_data) {
        // Valid envelope: __sv is a u64 and data is present.
        // Take ownership of the data field via remove() to avoid cloning.
        (_, Some(v), true) => {
            let v = u32::try_from(v)
                .map_err(|_| SysError::ApiError("schema version exceeds u32::MAX".into()))?;
            // Safety: the match guard confirmed has_data=true, so
            // value is an object with a "data" key. This is infallible.
            let data = value
                .as_object_mut()
                .and_then(|m| m.remove("data"))
                .expect("data field guaranteed by match condition");
            if v == current_version {
                let parsed: T = serde_json::from_value(data)?;
                Ok(Versioned::Current(parsed))
            } else if v < current_version {
                Ok(Versioned::NeedsMigration {
                    raw: data,
                    stored_version: v,
                })
            } else {
                Err(SysError::ApiError(format!(
                    "stored schema version {v} is newer than current \
                     version {current_version} - cannot safely read"
                )))
            }
        }
        // Malformed envelope: __sv present but data missing or __sv not a number.
        (true, _, _) => Err(SysError::ApiError(
            "malformed versioned envelope: __sv field present but \
             data field missing or __sv is not a number"
                .into(),
        )),
        // No __sv field at all: plain unversioned data.
        (false, _, _) => Ok(Versioned::Unversioned(value)),
    }
}

/// Read versioned data, automatically migrating older versions.
///
/// `migrate_fn` receives the raw JSON and the stored version, and must
/// return a `T` at `current_version`. The migrated value is automatically
/// saved back to KV.
///
/// **Warning:** The original data is overwritten after a successful
/// migration. If the write-back fails, the original data is preserved
/// and the migration will be re-attempted on the next call. Ensure
/// `migrate_fn` is idempotent and correct - there is no rollback
/// after a successful write.
///
/// For [`Versioned::Unversioned`] data, `migrate_fn` is called with
/// version 0. For [`Versioned::NotFound`], returns `None`.
pub fn get_versioned_or_migrate<T: Serialize + DeserializeOwned>(
    key: &str,
    current_version: u32,
    migrate_fn: impl FnOnce(serde_json::Value, u32) -> Result<T, SysError>,
) -> Result<Option<T>, SysError> {
    match get_versioned::<T>(key, current_version)? {
        Versioned::Current(data) => Ok(Some(data)),
        Versioned::NeedsMigration {
            raw,
            stored_version,
        } => {
            let migrated = migrate_fn(raw, stored_version)?;
            set_versioned(key, &migrated, current_version)?;
            Ok(Some(migrated))
        }
        Versioned::Unversioned(raw) => {
            let migrated = migrate_fn(raw, 0)?;
            set_versioned(key, &migrated, current_version)?;
            Ok(Some(migrated))
        }
        Versioned::NotFound => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestData {
        name: String,
        count: u32,
    }

    // ---- Envelope serialization tests ----

    #[test]
    fn versioned_envelope_roundtrip() {
        let envelope = VersionedEnvelope {
            schema_version: 1,
            data: TestData {
                name: "hello".into(),
                count: 42,
            },
        };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains("\"__sv\":1"));
        assert!(json.contains("\"data\":{"));

        let parsed: VersionedEnvelope<TestData> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(
            parsed.data,
            TestData {
                name: "hello".into(),
                count: 42,
            }
        );
    }

    #[test]
    fn versioned_envelope_wire_format() {
        let envelope = VersionedEnvelope {
            schema_version: 3,
            data: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["__sv"], 3);
        assert_eq!(parsed["data"]["key"], "value");
    }

    // ---- parse_versioned logic tests ----

    #[test]
    fn parse_versioned_empty_bytes_returns_not_found() {
        let result = parse_versioned::<TestData>(b"", 1).unwrap();
        assert!(matches!(result, Versioned::NotFound));
    }

    #[test]
    fn parse_versioned_current_version_returns_current() {
        let bytes = br#"{"__sv":2,"data":{"name":"hello","count":42}}"#;
        let result = parse_versioned::<TestData>(bytes, 2).unwrap();
        match result {
            Versioned::Current(data) => {
                assert_eq!(data.name, "hello");
                assert_eq!(data.count, 42);
            }
            other => panic!("expected Current, got {other:?}"),
        }
    }

    #[test]
    fn parse_versioned_older_version_returns_needs_migration() {
        let bytes = br#"{"__sv":1,"data":{"name":"old","count":1}}"#;
        let result = parse_versioned::<TestData>(bytes, 3).unwrap();
        match result {
            Versioned::NeedsMigration {
                raw,
                stored_version,
            } => {
                assert_eq!(stored_version, 1);
                assert_eq!(raw["name"], "old");
                assert_eq!(raw["count"], 1);
            }
            other => panic!("expected NeedsMigration, got {other:?}"),
        }
    }

    #[test]
    fn parse_versioned_newer_version_returns_error() {
        let bytes = br#"{"__sv":5,"data":{"name":"future","count":0}}"#;
        let result = parse_versioned::<TestData>(bytes, 2);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("newer than current"),
            "error should mention newer version: {err}"
        );
    }

    #[test]
    fn parse_versioned_plain_json_returns_unversioned() {
        let bytes = br#"{"name":"legacy","count":99}"#;
        let result = parse_versioned::<TestData>(bytes, 1).unwrap();
        match result {
            Versioned::Unversioned(val) => {
                assert_eq!(val["name"], "legacy");
                assert_eq!(val["count"], 99);
            }
            other => panic!("expected Unversioned, got {other:?}"),
        }
    }

    #[test]
    fn parse_versioned_malformed_sv_without_data_returns_error() {
        let bytes = br#"{"__sv":1,"payload":"something"}"#;
        let result = parse_versioned::<TestData>(bytes, 1);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("malformed"),
            "error should mention malformed envelope: {err}"
        );
    }

    #[test]
    fn parse_versioned_non_numeric_sv_returns_error() {
        let bytes = br#"{"__sv":"one","data":{}}"#;
        let result = parse_versioned::<TestData>(bytes, 1);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("malformed"),
            "error should mention malformed envelope: {err}"
        );
    }

    #[test]
    fn parse_versioned_version_zero_is_valid() {
        // Version 0 is a legitimate version (initial schema).
        let bytes = br#"{"__sv":0,"data":{"name":"v0","count":0}}"#;
        let result = parse_versioned::<TestData>(bytes, 0).unwrap();
        assert!(matches!(result, Versioned::Current(_)));
    }

    #[test]
    fn parse_versioned_invalid_json_returns_error() {
        let result = parse_versioned::<TestData>(b"not json", 1);
        assert!(result.is_err());
    }
}
