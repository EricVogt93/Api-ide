use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::model::RequestDef;
use crate::vars::{rename, spans, VarScopes};

use super::{
    save_collection_meta, save_environment, save_folder_meta, save_request, save_secrets,
    CollectionNode, StoreError, TreeNode, Workspace,
};

#[derive(Debug, thiserror::Error)]
pub enum VariableRenameError {
    #[error("invalid variable name {0:?}")]
    InvalidName(String),
    #[error("variable {0:?} already exists")]
    Conflict(String),
    #[error("variable {0:?} was not found")]
    NotFound(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("failed to transform request data: {0}")]
    Json(#[from] serde_json::Error),
}

/// Names of every `{{variable}}` reference in one request.
pub fn request_variable_names(
    request: &RequestDef,
) -> Result<BTreeSet<String>, VariableRenameError> {
    Ok(request_variable_counts(request)?.into_keys().collect())
}

/// Exact `{{name}}` reference counts in one request.
pub fn request_variable_counts(
    request: &RequestDef,
) -> Result<BTreeMap<String, usize>, VariableRenameError> {
    let value = serde_json::to_value(request)?;
    let mut counts = BTreeMap::new();
    visit_strings(&value, &mut |text| {
        for span in spans(text, &VarScopes::new()) {
            *counts.entry(span.name).or_default() += 1;
        }
    });
    Ok(counts)
}

/// Number of exact `{{name}}` references in one request.
pub fn request_variable_occurrences(
    request: &RequestDef,
    name: &str,
) -> Result<usize, VariableRenameError> {
    Ok(request_variable_counts(request)?
        .get(name)
        .copied()
        .unwrap_or(0))
}

/// Rename one template variable across definitions and references, then
/// persist only changed workspace files.
// ponytail: script API references such as vars.get("name") are not rewritten;
// add language-aware script refactoring if those become first-class symbols.
pub fn rename_workspace_variable(
    workspace: &mut Workspace,
    old: &str,
    new: &str,
) -> Result<usize, VariableRenameError> {
    validate_name(old)?;
    validate_name(new)?;
    if old == new {
        return Ok(0);
    }
    if has_definition(workspace, new) || workspace_occurrences(workspace, new)? > 0 {
        return Err(VariableRenameError::Conflict(new.to_string()));
    }

    let before = workspace.clone();
    let mut next = before.clone();
    let mut changed = rename_definitions(&mut next, old, new);
    changed += rename_collection_references(&mut next.collections, old, new)?;
    if changed == 0 {
        return Err(VariableRenameError::NotFound(old.to_string()));
    }

    persist_changes(&before, &next)?;
    *workspace = next;
    Ok(changed)
}

fn validate_name(name: &str) -> Result<(), VariableRenameError> {
    let mut chars = name.chars();
    let valid_first = chars
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_');
    let valid_rest = chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if valid_first && valid_rest {
        Ok(())
    } else {
        Err(VariableRenameError::InvalidName(name.to_string()))
    }
}

fn visit_strings(value: &Value, visitor: &mut impl FnMut(&str)) {
    match value {
        Value::String(text) => visitor(text),
        Value::Array(items) => {
            for item in items {
                visit_strings(item, visitor);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                visit_strings(value, visitor);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn rename_string_values<T>(
    value: &T,
    old: &str,
    new: &str,
) -> Result<(T, usize), VariableRenameError>
where
    T: Serialize + DeserializeOwned,
{
    let mut json = serde_json::to_value(value)?;
    let mut count = 0;
    rename_json_strings(&mut json, old, new, &mut count);
    Ok((serde_json::from_value(json)?, count))
}

fn rename_json_strings(value: &mut Value, old: &str, new: &str, count: &mut usize) {
    match value {
        Value::String(text) => {
            let (renamed, replacements) = rename(text, old, new);
            *text = renamed;
            *count += replacements;
        }
        Value::Array(items) => {
            for item in items {
                rename_json_strings(item, old, new, count);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                rename_json_strings(value, old, new, count);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn workspace_occurrences(workspace: &Workspace, name: &str) -> Result<usize, VariableRenameError> {
    let mut count = 0;
    for collection in &workspace.collections {
        count += serializable_occurrences(&collection.meta, name)?;
        count += node_occurrences(&collection.children, name)?;
    }
    Ok(count)
}

fn serializable_occurrences(
    value: &impl Serialize,
    name: &str,
) -> Result<usize, VariableRenameError> {
    let value = serde_json::to_value(value)?;
    let mut count = 0;
    visit_strings(&value, &mut |text| {
        count += spans(text, &VarScopes::new())
            .iter()
            .filter(|span| span.name == name)
            .count();
    });
    Ok(count)
}

fn node_occurrences(children: &[TreeNode], name: &str) -> Result<usize, VariableRenameError> {
    let mut count = 0;
    for child in children {
        match child {
            TreeNode::Request(request) => {
                count += request_variable_occurrences(&request.def, name)?
            }
            TreeNode::Folder(folder) => {
                count += serializable_occurrences(&folder.meta, name)?;
                count += node_occurrences(&folder.children, name)?;
            }
        }
    }
    Ok(count)
}

fn has_definition(workspace: &Workspace, name: &str) -> bool {
    workspace
        .environments
        .iter()
        .any(|loaded| loaded.env.variables.contains_key(name) || loaded.secrets.contains_key(name))
        || workspace.collections.iter().any(|collection| {
            collection.meta.variables.contains_key(name)
                || nodes_have_definition(&collection.children, name)
        })
}

fn nodes_have_definition(children: &[TreeNode], name: &str) -> bool {
    children.iter().any(|child| match child {
        TreeNode::Request(request) => request.def.extractors.iter().any(|ext| ext.var == name),
        TreeNode::Folder(folder) => {
            folder.meta.variables.contains_key(name)
                || nodes_have_definition(&folder.children, name)
        }
    })
}

fn rename_key<T>(map: &mut BTreeMap<String, T>, old: &str, new: &str) -> bool {
    let Some(value) = map.remove(old) else {
        return false;
    };
    map.insert(new.to_string(), value);
    true
}

fn rename_definitions(workspace: &mut Workspace, old: &str, new: &str) -> usize {
    let mut changed = 0;
    for loaded in &mut workspace.environments {
        changed += usize::from(rename_key(&mut loaded.env.variables, old, new));
        changed += usize::from(rename_key(&mut loaded.secrets, old, new));
    }
    for collection in &mut workspace.collections {
        changed += usize::from(rename_key(&mut collection.meta.variables, old, new));
        changed += rename_node_definitions(&mut collection.children, old, new);
    }
    changed
}

fn rename_node_definitions(children: &mut [TreeNode], old: &str, new: &str) -> usize {
    let mut changed = 0;
    for child in children {
        match child {
            TreeNode::Request(request) => {
                for extractor in &mut request.def.extractors {
                    if extractor.var == old {
                        extractor.var = new.to_string();
                        changed += 1;
                    }
                }
            }
            TreeNode::Folder(folder) => {
                changed += usize::from(rename_key(&mut folder.meta.variables, old, new));
                changed += rename_node_definitions(&mut folder.children, old, new);
            }
        }
    }
    changed
}

fn rename_collection_references(
    collections: &mut [CollectionNode],
    old: &str,
    new: &str,
) -> Result<usize, VariableRenameError> {
    let mut changed = 0;
    for collection in collections {
        let (meta, count) = rename_string_values(&collection.meta, old, new)?;
        collection.meta = meta;
        changed += count;
        changed += rename_node_references(&mut collection.children, old, new)?;
    }
    Ok(changed)
}

fn rename_node_references(
    children: &mut [TreeNode],
    old: &str,
    new: &str,
) -> Result<usize, VariableRenameError> {
    let mut changed = 0;
    for child in children {
        match child {
            TreeNode::Request(request) => {
                let (def, count) = rename_string_values(&request.def, old, new)?;
                request.def = def;
                changed += count;
            }
            TreeNode::Folder(folder) => {
                let (meta, count) = rename_string_values(&folder.meta, old, new)?;
                folder.meta = meta;
                changed += count;
                changed += rename_node_references(&mut folder.children, old, new)?;
            }
        }
    }
    Ok(changed)
}

fn persist_changes(before: &Workspace, after: &Workspace) -> Result<(), StoreError> {
    for (old, new) in before.environments.iter().zip(&after.environments) {
        if old.env != new.env {
            save_environment(&new.file, &new.env)?;
        }
        if old.secrets != new.secrets {
            save_secrets(&new.file, &new.secrets)?;
        }
    }
    for (old, new) in before.collections.iter().zip(&after.collections) {
        if old.meta != new.meta {
            save_collection_meta(&new.dir, &new.meta)?;
        }
        persist_node_changes(&old.children, &new.children)?;
    }
    Ok(())
}

fn persist_node_changes(before: &[TreeNode], after: &[TreeNode]) -> Result<(), StoreError> {
    for (old, new) in before.iter().zip(after) {
        match (old, new) {
            (TreeNode::Request(old), TreeNode::Request(new)) if old.def != new.def => {
                save_request(&new.file, &new.def)?;
            }
            (TreeNode::Folder(old), TreeNode::Folder(new)) => {
                if old.meta != new.meta {
                    save_folder_meta(&new.dir, &new.meta)?;
                }
                persist_node_changes(&old.children, &new.children)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EnvVar, KeyValue, Method};
    use crate::store::{
        create_collection, create_environment, create_request, load_json, save_collection_meta,
        save_environment,
    };

    #[test]
    fn workspace_rename_updates_definitions_references_and_disk() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        Workspace::create(root, "Test").unwrap();

        let env_file = create_environment(root, "dev").unwrap();
        let mut env = load_json::<crate::model::Environment>(&env_file).unwrap();
        env.variables
            .insert("baseUrl".into(), EnvVar::plain("https://example.test"));
        env.variables.insert("other".into(), EnvVar::plain("x"));
        save_environment(&env_file, &env).unwrap();

        let collection_dir = create_collection(root, "API").unwrap();
        let mut collection = load_json::<crate::model::CollectionMeta>(
            &collection_dir.join(super::super::COLLECTION_FILE),
        )
        .unwrap();
        collection
            .variables
            .insert("baseUrl".into(), "http://fallback.test".into());
        save_collection_meta(&collection_dir, &collection).unwrap();

        let mut request =
            RequestDef::new("Get user", Method::Get, "{{baseUrl}}/users/{{ userId }}");
        request
            .headers
            .push(KeyValue::new("X-Origin", "{{baseUrl}}"));
        let file = create_request(&collection_dir, &request).unwrap();

        let mut workspace = Workspace::load(root).unwrap();
        let request = workspace.all_requests()[0];
        assert_eq!(
            request_variable_names(&request.def).unwrap(),
            BTreeSet::from(["baseUrl".to_string(), "userId".to_string()])
        );
        assert_eq!(
            request_variable_occurrences(&request.def, "baseUrl").unwrap(),
            2
        );

        let conflict = rename_workspace_variable(&mut workspace, "baseUrl", "other").unwrap_err();
        assert!(matches!(conflict, VariableRenameError::Conflict(_)));

        assert_eq!(
            rename_workspace_variable(&mut workspace, "baseUrl", "apiBase").unwrap(),
            4
        );
        let reloaded = Workspace::load(root).unwrap();
        assert!(reloaded.environments[0]
            .env
            .variables
            .contains_key("apiBase"));
        assert!(reloaded.collections[0]
            .meta
            .variables
            .contains_key("apiBase"));
        let saved = std::fs::read_to_string(file).unwrap();
        assert!(saved.contains("{{apiBase}}/users/{{ userId }}"));
        assert!(saved.contains("{{apiBase}}"));
        assert!(!saved.contains("{{baseUrl}}"));
    }
}
