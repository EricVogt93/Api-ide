//! Thin wrapper around the `jsonschema` crate producing readable, collected
//! error strings instead of an error iterator tied to borrowed state.

use serde_json::Value;

/// Cap on how many individual validation errors are reported; schemas that
/// fail wildly (e.g. validating the wrong document) should not flood the UI.
const MAX_ERRORS: usize = 20;

/// Validate `instance` against `schema`.
///
/// Returns `Ok(())` when valid. Returns `Err` with up to
/// [`MAX_ERRORS`] human-readable `"<instance path>: <message>"` strings
/// when invalid, or a single-element `Err` if `schema` itself does not
/// compile as a valid JSON Schema.
pub fn validate(schema: &Value, instance: &Value) -> Result<(), Vec<String>> {
    let validator = match jsonschema::Validator::new(schema) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("invalid JSON schema: {e}")]),
    };

    let errors: Vec<String> = validator
        .iter_errors(instance)
        .take(MAX_ERRORS)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn valid_instance_passes() {
        let schema = json!({
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "number" } }
        });
        let instance = json!({ "id": 1 });
        assert_eq!(validate(&schema, &instance), Ok(()));
    }

    #[test]
    fn invalid_instance_collects_errors() {
        let schema = json!({
            "type": "object",
            "required": ["id", "name"],
            "properties": {
                "id": { "type": "number" },
                "name": { "type": "string" }
            }
        });
        let instance = json!({ "id": "not-a-number" });
        let errors = validate(&schema, &instance).unwrap_err();
        assert!(!errors.is_empty());
        assert!(errors.len() <= MAX_ERRORS);
        // At least one error should mention the `id` field's path.
        assert!(errors.iter().any(|e| e.contains("id")));
    }

    #[test]
    fn invalid_schema_itself_reports_single_error() {
        // `type` must be a string or array of strings, not a number.
        let schema = json!({ "type": 123 });
        let result = validate(&schema, &json!({}));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().len(), 1);
    }

    #[test]
    fn many_errors_are_capped() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string" },
                "b": { "type": "string" },
                "c": { "type": "string" },
            },
            "additionalProperties": false
        });
        // 30 unexpected properties -> way more than MAX_ERRORS violations.
        let mut map = serde_json::Map::new();
        for i in 0..30 {
            map.insert(format!("extra{i}"), json!(1));
        }
        let instance = Value::Object(map);
        let errors = validate(&schema, &instance).unwrap_err();
        assert!(errors.len() <= MAX_ERRORS);
    }
}
