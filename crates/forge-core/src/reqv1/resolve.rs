//! Data-asset resolution: load a JSON asset, apply a JSON Pointer, apply
//! request-local JSON Patch, with clone-on-read caching and reference-cycle
//! detection. See `docs/architecture/request-format-v1.md` §6.

use std::cell::RefCell;
use std::collections::HashMap;

use json_patch::PatchOperation;
use serde_json::Value;

use super::diag::{Code, Diagnostic};
use super::refs::{AssetDescriptor, RefResolver, RefScheme};

/// Loads and caches parsed data assets for one run. Clone-on-read guarantees
/// a request-local `patch` never leaks into another binding referencing the
/// same asset.
pub struct DataStore<'a> {
    resolver: &'a RefResolver,
    cache: RefCell<HashMap<String, Value>>,
    /// Resolution stack of `address#pointer` frames for cycle detection.
    stack: RefCell<Vec<String>>,
}

impl<'a> DataStore<'a> {
    pub fn new(resolver: &'a RefResolver) -> Self {
        Self { resolver, cache: RefCell::new(HashMap::new()), stack: RefCell::new(Vec::new()) }
    }

    /// Resolve a `ref` descriptor to a JSON value: load, pointer, patch.
    /// (Variable interpolation of the result is the caller's job, §6 step 8.)
    pub fn resolve(
        &self,
        desc: &AssetDescriptor,
        patch: &[PatchOperation],
    ) -> Result<Value, Diagnostic> {
        if desc.scheme != RefScheme::File {
            return Err(Diagnostic::new(
                Code::IncompatibleAssetType,
                format!("{:?} is not a data asset (use it in a pipeline/generator instead)", desc.raw),
            )
            .with_ref(&desc.raw));
        }

        let frame = format!("{}#{}", desc.address, desc.pointer.as_deref().unwrap_or(""));
        if self.stack.borrow().contains(&frame) {
            let mut chain = self.stack.borrow().clone();
            chain.push(frame.clone());
            return Err(Diagnostic::new(
                Code::ReferenceCycle,
                format!("reference cycle: {}", chain.join(" -> ")),
            )
            .with_ref(&desc.raw));
        }
        self.stack.borrow_mut().push(frame);
        let result = self.resolve_inner(desc, patch);
        self.stack.borrow_mut().pop();
        result
    }

    fn resolve_inner(
        &self,
        desc: &AssetDescriptor,
        patch: &[PatchOperation],
    ) -> Result<Value, Diagnostic> {
        // Load (cache by absolute path).
        if !self.cache.borrow().contains_key(&desc.address) {
            let text = std::fs::read_to_string(&desc.address).map_err(|e| {
                let code = if e.kind() == std::io::ErrorKind::NotFound {
                    Code::AssetNotFound
                } else {
                    Code::InvalidAssetInput
                };
                Diagnostic::new(code, format!("cannot read asset {}: {e}", desc.address))
                    .with_ref(&desc.raw)
            })?;
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                Diagnostic::new(
                    Code::InvalidAssetInput,
                    format!("asset {} is not valid JSON: {e}", desc.address),
                )
                .with_ref(&desc.raw)
            })?;
            // Optional sibling schema validation.
            self.validate_sibling_schema(&desc.address, &value, &desc.raw)?;
            self.cache.borrow_mut().insert(desc.address.clone(), value);
        }

        // Apply JSON Pointer on a fresh clone.
        let selected = {
            let cache = self.cache.borrow();
            let doc = &cache[&desc.address];
            match &desc.pointer {
                None => doc.clone(),
                Some(ptr) => doc
                    .pointer(ptr)
                    .cloned()
                    .ok_or_else(|| {
                        Diagnostic::new(
                            Code::InvalidPointer,
                            format!("JSON Pointer {ptr:?} selects nothing in {}", desc.address),
                        )
                        .with_ref(&desc.raw)
                    })?,
            }
        };

        // Apply request-local JSON Patch (RFC 6902), reporting the op index.
        if patch.is_empty() {
            return Ok(selected);
        }
        let mut patched = selected;
        for (i, op) in patch.iter().enumerate() {
            let single = [op.clone()];
            json_patch::patch(&mut patched, &single).map_err(|e| {
                Diagnostic::new(
                    Code::JsonPatchFailed,
                    format!("patch op #{i} failed on {}: {e}", desc.raw),
                )
                .with_ref(&desc.raw)
            })?;
        }
        Ok(patched)
    }

    /// If `<asset>.schema.json` exists next to a `<asset>.json`, validate. A
    /// v1 pragmatic check: presence + JSON parse of the schema. (Full
    /// draft-2020-12 validation is an extension point; keeping it light avoids
    /// a heavy validator dep in core for the first version.)
    fn validate_sibling_schema(
        &self,
        asset_path: &str,
        value: &Value,
        raw: &str,
    ) -> Result<(), Diagnostic> {
        let Some(schema_path) = asset_path.strip_suffix(".json").map(|p| format!("{p}.schema.json"))
        else {
            return Ok(());
        };
        if !std::path::Path::new(&schema_path).exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&schema_path).map_err(|e| {
            Diagnostic::new(Code::InvalidAssetInput, format!("cannot read schema {schema_path}: {e}"))
                .with_ref(raw)
        })?;
        let schema: Value = serde_json::from_str(&text).map_err(|e| {
            Diagnostic::new(
                Code::InvalidAssetInput,
                format!("sibling schema {schema_path} is not valid JSON: {e}"),
            )
            .with_ref(raw)
        })?;
        // Full draft-2020-12 validation of the whole data document against
        // its sibling schema, before any JSON Pointer is applied.
        crate::assert::schema::validate(&schema, value).map_err(|errors| {
            Diagnostic::new(
                Code::InvalidAssetInput,
                format!(
                    "asset {asset_path} violates {schema_path}: {}",
                    errors.join("; ")
                ),
            )
            .with_ref(raw)
        })
    }

    pub fn resolver(&self) -> &RefResolver {
        self.resolver
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::model::ProjectConfig;
    use std::path::Path;

    fn setup(files: &[(&str, &str)]) -> (tempfile::TempDir, RefResolver) {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let project: ProjectConfig = serde_json::from_str(
            r#"{"aliases":{"data:users":"./users.json","data:a":"./a.json","data:b":"./b.json"}}"#,
        )
        .unwrap();
        let resolver = RefResolver::new(dir.path(), &project).unwrap();
        (dir, resolver)
    }

    #[test]
    fn pointer_selects_subtree() {
        let (dir, r) = setup(&[("users.json", r#"{"valid":{"alice":{"name":"Alice"}}}"#)]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/valid/alice", dir.path()).unwrap();
        let v = store.resolve(&d, &[]).unwrap();
        assert_eq!(v, serde_json::json!({ "name": "Alice" }));
    }

    #[test]
    fn missing_pointer_errors() {
        let (dir, r) = setup(&[("users.json", r#"{"valid":{}}"#)]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/valid/nobody", dir.path()).unwrap();
        assert_eq!(store.resolve(&d, &[]).unwrap_err().code, Code::InvalidPointer.as_str());
    }

    #[test]
    fn missing_asset_errors() {
        let (dir, r) = setup(&[("users.json", "{}")]);
        // Alias data:a points at ./a.json which does not exist.
        let store = DataStore::new(&r);
        let d = r.resolve("data:a#/x", dir.path()).unwrap();
        assert_eq!(store.resolve(&d, &[]).unwrap_err().code, Code::AssetNotFound.as_str());
    }

    #[test]
    fn json_patch_applies_and_reports_failure() {
        let (dir, r) = setup(&[("users.json", r#"{"valid":{"alice":{"email":"a@x"}}}"#)]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/valid/alice", dir.path()).unwrap();

        let ok: Vec<PatchOperation> =
            serde_json::from_str(r#"[{"op":"replace","path":"/email","value":"z@x"}]"#).unwrap();
        let v = store.resolve(&d, &ok).unwrap();
        assert_eq!(v, serde_json::json!({ "email": "z@x" }));

        let bad: Vec<PatchOperation> =
            serde_json::from_str(r#"[{"op":"replace","path":"/nope","value":1}]"#).unwrap();
        let err = store.resolve(&d, &bad).unwrap_err();
        assert_eq!(err.code, Code::JsonPatchFailed.as_str());
    }

    #[test]
    fn patch_does_not_leak_into_cache() {
        let (dir, r) = setup(&[("users.json", r#"{"x":{"n":1}}"#)]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/x", dir.path()).unwrap();
        let patch: Vec<PatchOperation> =
            serde_json::from_str(r#"[{"op":"replace","path":"/n","value":99}]"#).unwrap();
        let _ = store.resolve(&d, &patch).unwrap();
        // A second, unpatched resolve must see the original value.
        let clean = store.resolve(&d, &[]).unwrap();
        assert_eq!(clean, serde_json::json!({ "n": 1 }));
    }

    #[test]
    fn sibling_schema_bad_json_errors() {
        let (dir, r) = setup(&[
            ("users.json", r#"{"x":1}"#),
            ("users.schema.json", "{ not json"),
        ]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/x", dir.path()).unwrap();
        assert_eq!(store.resolve(&d, &[]).unwrap_err().code, Code::InvalidAssetInput.as_str());
    }

    #[test]
    fn sibling_schema_validates_the_data() {
        let schema = r#"{"type":"object","required":["valid"],
            "properties":{"valid":{"type":"object"}}}"#;
        // Passes its schema.
        let (dir, r) = setup(&[
            ("users.json", r#"{"valid":{"alice":{}}}"#),
            ("users.schema.json", schema),
        ]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/valid/alice", dir.path()).unwrap();
        assert!(store.resolve(&d, &[]).is_ok());

        // Violates its schema (missing required "valid").
        let (dir, r) = setup(&[
            ("users.json", r#"{"other":1}"#),
            ("users.schema.json", schema),
        ]);
        let store = DataStore::new(&r);
        let d = r.resolve("data:users#/other", dir.path()).unwrap();
        let err = store.resolve(&d, &[]).unwrap_err();
        assert_eq!(err.code, Code::InvalidAssetInput.as_str());
        assert!(err.message.contains("violates"), "{}", err.message);
    }

    #[test]
    fn builtin_ref_is_incompatible_as_data() {
        let (dir, r) = setup(&[("users.json", "{}")]);
        let _ = dir;
        let store = DataStore::new(&r);
        let d = r.resolve("builtin:uuid@1", Path::new("/")).unwrap();
        assert_eq!(store.resolve(&d, &[]).unwrap_err().code, Code::IncompatibleAssetType.as_str());
    }
}
