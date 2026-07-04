use serde::{Deserialize, Serialize};

use super::ScriptLang;

/// Suite lifecycle hooks attached to a collection or a folder: scripts that
/// run around the requests underneath it, independent of any individual
/// request's own pre-request/post-response scripts.
///
/// `before_all`/`after_all` run once per run (not once per data-driven
/// iteration); `before_each`/`after_each` run around every request. See
/// `runner::run` for the full ordering and error-handling rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SuiteHooks {
    /// Runs once, before the first request under this collection/folder
    /// executes. Sees `vars`/`log`/`assert`/helpers; no `req`/`res`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_all: Option<String>,
    /// Runs before every request under this collection/folder. Same host
    /// API as `before_all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_each: Option<String>,
    /// Runs after every request under this collection/folder. Additionally
    /// sees `res`, the just-finished request's response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_each: Option<String>,
    /// Runs once, after the last request under this collection/folder
    /// executes. Additionally sees `res`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_all: Option<String>,
    /// Scripting language all four hooks above are written in.
    #[serde(default, skip_serializing_if = "is_default_lang")]
    pub language: ScriptLang,
}

fn is_default_lang(lang: &ScriptLang) -> bool {
    *lang == ScriptLang::default()
}

impl SuiteHooks {
    /// `true` when no hook script is set (the `language` field alone never
    /// keeps a `SuiteHooks` from being considered empty).
    pub fn is_empty(&self) -> bool {
        self.before_all.is_none()
            && self.before_each.is_none()
            && self.after_each.is_none()
            && self.after_all.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CollectionMeta, FolderMeta};

    #[test]
    fn suite_hooks_round_trip_on_collection_meta() {
        let mut meta = CollectionMeta::new("C");
        meta.hooks = SuiteHooks {
            before_all: Some("vars.set(\"a\", \"1\");".to_string()),
            after_each: Some("assert(res.status < 500, \"no 5xx\");".to_string()),
            language: ScriptLang::Js,
            ..Default::default()
        };
        let json = serde_json::to_string(&meta).expect("serialize");
        assert!(json.contains(r#""beforeAll""#), "{json}");
        assert!(json.contains(r#""language":"js""#), "{json}");
        let back: CollectionMeta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.hooks, meta.hooks);
    }

    #[test]
    fn empty_hooks_are_omitted_and_legacy_meta_parses() {
        let meta = CollectionMeta::new("C");
        let json = serde_json::to_string(&meta).expect("serialize");
        assert!(!json.contains("hooks"), "empty hooks must be omitted: {json}");

        let folder: FolderMeta = serde_json::from_str(r#"{"name":"F"}"#).expect("deserialize");
        assert!(folder.hooks.is_empty());
    }

    #[test]
    fn hooks_with_only_language_set_still_count_as_empty() {
        let hooks = SuiteHooks { language: ScriptLang::Js, ..Default::default() };
        assert!(hooks.is_empty());
    }
}
