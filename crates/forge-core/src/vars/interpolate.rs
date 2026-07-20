//! `{{name}}` interpolation and span extraction for editor highlighting.
//!
//! Syntax is intentionally minimal: `{{` starts a variable reference and
//! whitespace directly inside the braces (`{{ name }}`) is ignored. There is
//! no escaping — `{{{{` is not special, it is just two consecutive `{{`
//! reference starts. A `{{` with no matching `}}` later in the template is
//! left verbatim (not treated as a reference).

use super::scope::VarScopes;

/// A single `{{name}}` occurrence found in a template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarSpan {
    /// Byte offset of the opening `{{` in the template.
    pub start: usize,
    /// Byte offset just past the closing `}}` in the template.
    pub end: usize,
    /// The trimmed variable name (without braces or surrounding whitespace).
    pub name: String,
    /// The resolved value, or `None` if the variable could not be resolved.
    /// Extraction never errors — unresolved variables just carry `None`.
    pub resolved: Option<String>,
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InterpolateError {
    /// One or more `{{name}}` references could not be resolved. Carries
    /// every unresolved name found (not just the first), in order of
    /// appearance, duplicates included.
    #[error("unresolved variable(s): {}", .names.join(", "))]
    Unresolved { names: Vec<String> },
}

/// A raw `{{ name }}` reference found by scanning a template.
struct RawRef<'a> {
    start: usize,
    end: usize,
    name: &'a str,
}

/// Scan `template` for `{{ name }}` references. Byte-offset based, respects
/// UTF-8 boundaries (the surrounding braces are always ASCII, so any inner
/// multibyte content is passed through untouched).
fn scan(template: &str) -> Vec<RawRef<'_>> {
    let bytes = template.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let search_start = i + 2;
            if let Some(rel_end) = template[search_start..].find("}}") {
                let inner_end = search_start + rel_end;
                let end = inner_end + 2;
                let name = template[search_start..inner_end].trim();
                refs.push(RawRef {
                    start: i,
                    end,
                    name,
                });
                i = end;
                continue;
            } else {
                // Unmatched "{{": leave verbatim, keep scanning past it.
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    refs
}

/// Interpolate every `{{name}}` reference in `template` against `scopes`.
///
/// Resolved values are substituted verbatim and are **not** recursively
/// re-expanded, even if they themselves contain `{{...}}`. If any reference
/// cannot be resolved, returns `Err` collecting *all* unresolved names
/// rather than stopping at the first.
pub fn interpolate(template: &str, scopes: &VarScopes) -> Result<String, InterpolateError> {
    let refs = scan(template);
    if refs.is_empty() {
        return Ok(template.to_string());
    }

    let mut unresolved = Vec::new();
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0usize;
    for r in &refs {
        out.push_str(&template[cursor..r.start]);
        match scopes.lookup(r.name) {
            Some(resolved) => out.push_str(&resolved.value),
            None => unresolved.push(r.name.to_string()),
        }
        cursor = r.end;
    }
    out.push_str(&template[cursor..]);

    if unresolved.is_empty() {
        Ok(out)
    } else {
        Err(InterpolateError::Unresolved { names: unresolved })
    }
}

/// Extract every `{{name}}` reference in `template` for editor highlighting
/// and hover previews. Never errors: unresolved references simply carry
/// `resolved: None`.
pub fn spans(template: &str, scopes: &VarScopes) -> Vec<VarSpan> {
    scan(template)
        .into_iter()
        .map(|r| match scopes.lookup(r.name) {
            Some(resolved) => VarSpan {
                start: r.start,
                end: r.end,
                name: r.name.to_string(),
                resolved: Some(resolved.value),
                secret: resolved.secret,
            },
            None => VarSpan {
                start: r.start,
                end: r.end,
                name: r.name.to_string(),
                resolved: None,
                secret: false,
            },
        })
        .collect()
}

/// Rename exact `{{name}}` references while preserving whitespace inside the
/// braces. Returns the rewritten template and the number of replacements.
pub fn rename(template: &str, old: &str, new: &str) -> (String, usize) {
    if old.is_empty() || old == new {
        return (template.to_string(), 0);
    }
    let refs: Vec<_> = scan(template)
        .into_iter()
        .filter(|reference| reference.name == old)
        .collect();
    if refs.is_empty() {
        return (template.to_string(), 0);
    }

    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;
    for reference in &refs {
        out.push_str(&template[cursor..reference.start + 2]);
        let inner = &template[reference.start + 2..reference.end - 2];
        let leading = inner.len() - inner.trim_start().len();
        let trailing = inner.len() - inner.trim_end().len();
        out.push_str(&inner[..leading]);
        out.push_str(new);
        out.push_str(&inner[inner.len() - trailing..]);
        out.push_str("}}");
        cursor = reference.end;
    }
    out.push_str(&template[cursor..]);
    (out, refs.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn scopes_with(pairs: &[(&str, &str)]) -> VarScopes {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        VarScopes::new().with_collection(&map)
    }

    #[test]
    fn no_vars_fast_path_returns_same_text() {
        let scopes = VarScopes::new();
        assert_eq!(
            interpolate("plain text, no vars", &scopes).unwrap(),
            "plain text, no vars"
        );
        assert!(spans("plain text, no vars", &scopes).is_empty());
    }

    #[test]
    fn basic_substitution() {
        let scopes = scopes_with(&[("name", "world")]);
        assert_eq!(
            interpolate("hello {{name}}!", &scopes).unwrap(),
            "hello world!"
        );
    }

    #[test]
    fn whitespace_forms() {
        let scopes = scopes_with(&[("name", "world")]);
        for tpl in [
            "{{name}}",
            "{{ name}}",
            "{{name }}",
            "{{  name  }}",
            "{{\tname\t}}",
        ] {
            assert_eq!(
                interpolate(tpl, &scopes).unwrap(),
                "world",
                "template: {tpl:?}"
            );
        }
    }

    #[test]
    fn adjacent_vars() {
        let scopes = scopes_with(&[("a", "1"), ("b", "2")]);
        assert_eq!(interpolate("{{a}}{{b}}", &scopes).unwrap(), "12");
    }

    #[test]
    fn unresolved_collects_all_names_in_order() {
        let scopes = VarScopes::new();
        let err = interpolate("{{a}} and {{b}} and {{a}}", &scopes).unwrap_err();
        match err {
            InterpolateError::Unresolved { names } => {
                assert_eq!(names, vec!["a", "b", "a"]);
            }
        }
    }

    #[test]
    fn mixed_resolved_and_unresolved_reports_only_unresolved() {
        let scopes = scopes_with(&[("known", "x")]);
        let err = interpolate("{{known}} {{missing}}", &scopes).unwrap_err();
        match err {
            InterpolateError::Unresolved { names } => assert_eq!(names, vec!["missing"]),
        }
    }

    #[test]
    fn no_recursive_expansion() {
        let scopes = scopes_with(&[("a", "{{b}}"), ("b", "final")]);
        // {{a}} resolves to the literal string "{{b}}", not re-expanded.
        assert_eq!(interpolate("{{a}}", &scopes).unwrap(), "{{b}}");
    }

    #[test]
    fn unmatched_open_brace_left_verbatim() {
        let scopes = VarScopes::new();
        assert_eq!(
            interpolate("just {{ opening", &scopes).unwrap(),
            "just {{ opening"
        );
    }

    #[test]
    fn dynamic_var_interpolates() {
        let scopes = VarScopes::new();
        let out = interpolate("{{$uuid}}", &scopes).unwrap();
        assert_eq!(out.len(), 36);
    }

    #[test]
    fn dynamic_var_reevaluates_per_occurrence() {
        let scopes = VarScopes::new();
        let out = interpolate("{{$uuid}}|{{$uuid}}", &scopes).unwrap();
        let (a, b) = out.split_once('|').unwrap();
        assert_eq!(a.len(), 36);
        assert_eq!(b.len(), 36);
        assert_ne!(a, b);
    }

    #[test]
    fn spans_basic_offsets() {
        let scopes = scopes_with(&[("name", "world")]);
        let tpl = "hi {{name}}!";
        let found = spans(tpl, &scopes);
        assert_eq!(found.len(), 1);
        let s = &found[0];
        assert_eq!(&tpl[s.start..s.end], "{{name}}");
        assert_eq!(s.name, "name");
        assert_eq!(s.resolved.as_deref(), Some("world"));
        assert!(!s.secret);
    }

    #[test]
    fn spans_multibyte_utf8_offsets() {
        let scopes = scopes_with(&[("name", "world")]);
        // "héllo " has a 2-byte 'é'; ensure offsets still land correctly.
        let tpl = "héllo {{name}} → done";
        let found = spans(tpl, &scopes);
        assert_eq!(found.len(), 1);
        let s = &found[0];
        assert_eq!(&tpl[s.start..s.end], "{{name}}");
        // Slicing at start/end must not panic (i.e. they are char boundaries)
        assert!(tpl.is_char_boundary(s.start));
        assert!(tpl.is_char_boundary(s.end));
    }

    #[test]
    fn spans_never_error_on_unresolved() {
        let scopes = VarScopes::new();
        let found = spans("{{missing}}", &scopes);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resolved, None);
        assert!(!found[0].secret);
    }

    #[test]
    fn rename_only_changes_exact_references_and_keeps_spacing() {
        let (renamed, count) = rename(
            "{{baseUrl}} {{ baseUrl }} {{baseUrlExtra}}",
            "baseUrl",
            "apiBase",
        );
        assert_eq!(renamed, "{{apiBase}} {{ apiBase }} {{baseUrlExtra}}");
        assert_eq!(count, 2);
    }

    #[test]
    fn spans_report_secret_flag() {
        use crate::model::{EnvVar, Environment, SecretValues};
        let mut env = Environment::new("e");
        env.variables.insert("token".into(), EnvVar::secret());
        let mut secrets = SecretValues::new();
        secrets.insert("token".into(), "s3cr3t".into());
        let scopes = VarScopes::new().with_environment(&env, &secrets);
        let found = spans("{{token}}", &scopes);
        assert_eq!(found[0].resolved.as_deref(), Some("s3cr3t"));
        assert!(found[0].secret);
    }

    #[test]
    fn spans_adjacent_vars_offsets() {
        let scopes = scopes_with(&[("a", "1"), ("b", "2")]);
        let tpl = "{{a}}{{b}}";
        let found = spans(tpl, &scopes);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].start, 0);
        assert_eq!(found[0].end, 5);
        assert_eq!(found[1].start, 5);
        assert_eq!(found[1].end, 10);
    }

    #[test]
    fn empty_template() {
        let scopes = VarScopes::new();
        assert_eq!(interpolate("", &scopes).unwrap(), "");
        assert!(spans("", &scopes).is_empty());
    }

    #[test]
    fn empty_name_is_unresolved() {
        let scopes = VarScopes::new();
        let err = interpolate("{{}}", &scopes).unwrap_err();
        match err {
            InterpolateError::Unresolved { names } => assert_eq!(names, vec![""]),
        }
    }
}
