//! Textual diffing of history entries — used to compare two responses
//! (e.g. before/after a code change, or across environments).

use similar::{ChangeTag, TextDiff};

use super::HistoryEntry;

/// Result of diffing two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffResult {
    /// Unified diff (3 lines of context, `--- a` / `+++ b` headers).
    /// Empty when the two texts are identical.
    pub unified: String,
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
}

/// Line-level unified diff between two texts.
pub fn diff_text(old: &str, new: &str) -> DiffResult {
    let diff = TextDiff::from_lines(old, new);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header("a", "b")
        .to_string();

    let mut added = 0usize;
    let mut removed = 0usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }

    DiffResult {
        unified,
        added,
        removed,
    }
}

/// Diff the response bodies of two history entries. Bodies that parse as
/// JSON are pretty-printed (with normalized key order/whitespace) before
/// diffing so that semantically-identical-but-differently-formatted
/// responses show as a harmless (empty) diff; other bodies are compared as
/// lossy UTF-8 text.
pub fn diff_entries(a: &HistoryEntry, b: &HistoryEntry) -> DiffResult {
    let old = normalize_body(a.response_body.as_deref());
    let new = normalize_body(b.response_body.as_deref());
    diff_text(&old, &new)
}

fn normalize_body(body: Option<&[u8]>) -> String {
    let Some(bytes) = body else {
        return String::new();
    };
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => serde_json::to_string_pretty(&canonicalize_json(value))
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned()),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Recursively sort object keys so that documents differing only in key
/// order serialize identically. `serde_json`'s `preserve_order` feature
/// (used elsewhere in the crate for on-disk file stability) otherwise keeps
/// the original, unsorted, insertion order.
fn canonicalize_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            let mut sorted = serde_json::Map::new();
            for (key, val) in entries {
                sorted.insert(key, canonicalize_json(val));
            }
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_json).collect())
        }
        other => other,
    }
}
