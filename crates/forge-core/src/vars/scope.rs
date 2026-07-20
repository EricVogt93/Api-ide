//! `VarScopes` — the variable resolution chain.

use std::collections::BTreeMap;

use crate::model::{Environment, SecretValues};

use super::dynamic;

/// Where a resolved variable's value came from, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarOrigin {
    /// A built-in dynamic variable such as `{{$uuid}}`.
    Dynamic,
    /// A data-driven run's current iteration row.
    Iteration,
    /// Extracted at runtime during the current run/session.
    Runtime,
    /// The active environment (secret values overlaid over plain values).
    Environment,
    /// A folder variable; nearer folders shadow farther ones.
    Folder,
    /// The owning collection's variables.
    Collection,
}

/// A variable value together with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVar {
    pub value: String,
    /// Whether the value should be treated as sensitive (redacted in logs
    /// and the UI).
    pub secret: bool,
    pub origin: VarOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvEntry {
    value: String,
    secret: bool,
}

/// Owns the full variable resolution chain for interpolating a request.
///
/// Priority, highest first:
/// 1. built-in dynamic variables (`{{$uuid}}`, …) — never stored, always
///    computed fresh by [`dynamic::resolve`];
/// 2. the current iteration's data-driven row;
/// 3. runtime variables extracted during the run/session;
/// 4. the active environment (secret values overlay plain ones);
/// 5. folder variables, nearest folder first;
/// 6. the owning collection's variables.
///
/// Built with a builder-style API; `set_runtime` / `set_iteration_row`
/// mutate in place since they change as a run progresses.
#[derive(Debug, Clone, Default)]
pub struct VarScopes {
    iteration: BTreeMap<String, String>,
    runtime: BTreeMap<String, String>,
    environment: BTreeMap<String, EnvEntry>,
    /// Index 0 = innermost (nearest) folder.
    folders: Vec<BTreeMap<String, String>>,
    collection: BTreeMap<String, String>,
}

impl VarScopes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge in an environment's variables. Plain variables use
    /// `EnvVar::value` directly; variables marked `secret` are looked up in
    /// `secrets` instead (and skipped entirely if no secret value is set).
    pub fn with_environment(mut self, env: &Environment, secrets: &SecretValues) -> Self {
        for (name, var) in &env.variables {
            if var.secret {
                if let Some(value) = secrets.get(name) {
                    self.environment.insert(
                        name.clone(),
                        EnvEntry {
                            value: value.clone(),
                            secret: true,
                        },
                    );
                }
            } else if let Some(value) = &var.value {
                self.environment.insert(
                    name.clone(),
                    EnvEntry {
                        value: value.clone(),
                        secret: false,
                    },
                );
            }
        }
        self
    }

    /// Merge in the owning collection's variables.
    pub fn with_collection(mut self, vars: &BTreeMap<String, String>) -> Self {
        self.collection
            .extend(vars.iter().map(|(k, v)| (k.clone(), v.clone())));
        self
    }

    /// Push one folder's variables onto the chain. Call from the innermost
    /// (nearest to the request) folder outward — the first call becomes the
    /// nearest scope.
    pub fn with_folder(mut self, vars: &BTreeMap<String, String>) -> Self {
        self.folders.push(vars.clone());
        self
    }

    /// Push several folders' variables at once, already ordered nearest
    /// (index 0) to farthest.
    pub fn with_folders<'a>(
        mut self,
        folders: impl IntoIterator<Item = &'a BTreeMap<String, String>>,
    ) -> Self {
        for vars in folders {
            self.folders.push(vars.clone());
        }
        self
    }

    /// Set (or overwrite) a single runtime variable, e.g. one extracted from
    /// a response during a run.
    pub fn set_runtime(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.runtime.insert(name.into(), value.into());
    }

    /// Replace the current data-driven iteration row wholesale.
    pub fn set_iteration_row(&mut self, row: BTreeMap<String, String>) {
        self.iteration = row;
    }

    /// Resolve a variable name (without the surrounding `{{ }}`) through the
    /// full priority chain. `name` should already be trimmed of whitespace.
    pub fn lookup(&self, name: &str) -> Option<ResolvedVar> {
        if let Some(value) = dynamic::resolve(name) {
            return Some(ResolvedVar {
                value,
                secret: false,
                origin: VarOrigin::Dynamic,
            });
        }
        if let Some(value) = self.iteration.get(name) {
            return Some(ResolvedVar {
                value: value.clone(),
                secret: false,
                origin: VarOrigin::Iteration,
            });
        }
        if let Some(value) = self.runtime.get(name) {
            return Some(ResolvedVar {
                value: value.clone(),
                secret: false,
                origin: VarOrigin::Runtime,
            });
        }
        if let Some(entry) = self.environment.get(name) {
            return Some(ResolvedVar {
                value: entry.value.clone(),
                secret: entry.secret,
                origin: VarOrigin::Environment,
            });
        }
        for folder in &self.folders {
            if let Some(value) = folder.get(name) {
                return Some(ResolvedVar {
                    value: value.clone(),
                    secret: false,
                    origin: VarOrigin::Folder,
                });
            }
        }
        if let Some(value) = self.collection.get(name) {
            return Some(ResolvedVar {
                value: value.clone(),
                secret: false,
                origin: VarOrigin::Collection,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EnvVar;

    fn env_with(pairs: &[(&str, EnvVar)]) -> Environment {
        let mut env = Environment::new("test");
        for (k, v) in pairs {
            env.variables.insert((*k).to_string(), v.clone());
        }
        env
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_scopes_resolve_nothing() {
        let scopes = VarScopes::new();
        assert!(scopes.lookup("missing").is_none());
    }

    #[test]
    fn dynamic_beats_everything() {
        let mut scopes = VarScopes::new().with_collection(&map(&[("x", "collection")]));
        scopes.set_runtime("x", "runtime");
        // $uuid isn't user-settable, but verify dynamic lookup still works
        // even with other scopes populated.
        assert!(scopes.lookup("$uuid").is_some());
        assert_eq!(scopes.lookup("x").unwrap().value, "runtime");
    }

    #[test]
    fn iteration_beats_runtime() {
        let mut scopes = VarScopes::new();
        scopes.set_runtime("x", "runtime");
        scopes.set_iteration_row(map(&[("x", "iteration")]));
        let r = scopes.lookup("x").unwrap();
        assert_eq!(r.value, "iteration");
        assert_eq!(r.origin, VarOrigin::Iteration);
    }

    #[test]
    fn runtime_beats_environment() {
        let mut secrets = SecretValues::new();
        secrets.insert("unused".into(), "x".into());
        let env = env_with(&[("x", EnvVar::plain("env"))]);
        let mut scopes = VarScopes::new().with_environment(&env, &secrets);
        scopes.set_runtime("x", "runtime");
        assert_eq!(scopes.lookup("x").unwrap().value, "runtime");
    }

    #[test]
    fn environment_beats_folder_and_collection() {
        let env = env_with(&[("x", EnvVar::plain("env"))]);
        let scopes = VarScopes::new()
            .with_environment(&env, &SecretValues::new())
            .with_folder(&map(&[("x", "folder")]))
            .with_collection(&map(&[("x", "collection")]));
        let r = scopes.lookup("x").unwrap();
        assert_eq!(r.value, "env");
        assert_eq!(r.origin, VarOrigin::Environment);
    }

    #[test]
    fn nearest_folder_wins() {
        let scopes = VarScopes::new()
            .with_folder(&map(&[("x", "inner")]))
            .with_folder(&map(&[("x", "outer")]));
        let r = scopes.lookup("x").unwrap();
        assert_eq!(r.value, "inner");
        assert_eq!(r.origin, VarOrigin::Folder);
    }

    #[test]
    fn with_folders_preserves_order() {
        let inner = map(&[("x", "inner")]);
        let outer = map(&[("x", "outer")]);
        let scopes = VarScopes::new().with_folders([&inner, &outer]);
        assert_eq!(scopes.lookup("x").unwrap().value, "inner");
    }

    #[test]
    fn folder_beats_collection() {
        let scopes = VarScopes::new()
            .with_folder(&map(&[("x", "folder")]))
            .with_collection(&map(&[("x", "collection")]));
        let r = scopes.lookup("x").unwrap();
        assert_eq!(r.value, "folder");
        assert_eq!(r.origin, VarOrigin::Folder);
    }

    #[test]
    fn collection_is_lowest_priority_but_resolves() {
        let scopes = VarScopes::new().with_collection(&map(&[("x", "collection")]));
        let r = scopes.lookup("x").unwrap();
        assert_eq!(r.value, "collection");
        assert_eq!(r.origin, VarOrigin::Collection);
        assert!(!r.secret);
    }

    #[test]
    fn secret_env_var_overlaid_from_secrets_map() {
        let env = env_with(&[("token", EnvVar::secret())]);
        let mut secrets = SecretValues::new();
        secrets.insert("token".into(), "s3cr3t".into());
        let scopes = VarScopes::new().with_environment(&env, &secrets);
        let r = scopes.lookup("token").unwrap();
        assert_eq!(r.value, "s3cr3t");
        assert!(r.secret);
        assert_eq!(r.origin, VarOrigin::Environment);
    }

    #[test]
    fn secret_env_var_without_secret_value_is_unresolved() {
        let env = env_with(&[("token", EnvVar::secret())]);
        let scopes = VarScopes::new().with_environment(&env, &SecretValues::new());
        assert!(scopes.lookup("token").is_none());
    }

    #[test]
    fn plain_env_var_is_not_secret() {
        let env = env_with(&[("x", EnvVar::plain("v"))]);
        let scopes = VarScopes::new().with_environment(&env, &SecretValues::new());
        let r = scopes.lookup("x").unwrap();
        assert!(!r.secret);
    }
}
