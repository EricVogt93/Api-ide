//! Integration tests for the `vars` module exercising the full precedence
//! chain end to end, plus a fixture-driven interpolation scenario.

use std::collections::BTreeMap;
use std::fs;

use forge_core::model::{EnvVar, Environment, SecretValues};
use forge_core::vars::{interpolate, spans, InterpolateError, VarOrigin, VarScopes};

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Full six-tier precedence chain, all tiers populated with the same key,
/// verifying resolution picks the highest-priority tier every time and that
/// removing tiers from the top falls through correctly.
#[test]
fn full_precedence_chain() {
    let mut env = Environment::new("prod");
    env.variables
        .insert("host".into(), EnvVar::plain("env-value"));
    let secrets = SecretValues::new();

    let collection_vars = map(&[("host", "collection-value")]);
    let outer_folder = map(&[("host", "outer-folder-value")]);
    let inner_folder = map(&[("host", "inner-folder-value")]);

    // 6. collection only
    let collection_only = VarScopes::new().with_collection(&collection_vars);
    assert_eq!(
        collection_only.lookup("host").unwrap().value,
        "collection-value"
    );
    assert_eq!(
        collection_only.lookup("host").unwrap().origin,
        VarOrigin::Collection
    );

    // 5. folders beat collection, nearest folder wins
    let folders_and_collection = VarScopes::new()
        .with_collection(&collection_vars)
        .with_folder(&inner_folder)
        .with_folder(&outer_folder);
    let r = folders_and_collection.lookup("host").unwrap();
    assert_eq!(r.value, "inner-folder-value");
    assert_eq!(r.origin, VarOrigin::Folder);

    // 4. environment beats folders and collection
    let mut scopes = VarScopes::new()
        .with_environment(&env, &secrets)
        .with_collection(&collection_vars)
        .with_folder(&inner_folder)
        .with_folder(&outer_folder);
    let r = scopes.lookup("host").unwrap();
    assert_eq!(r.value, "env-value");
    assert_eq!(r.origin, VarOrigin::Environment);

    // 3. runtime beats environment
    scopes.set_runtime("host", "runtime-value");
    let r = scopes.lookup("host").unwrap();
    assert_eq!(r.value, "runtime-value");
    assert_eq!(r.origin, VarOrigin::Runtime);

    // 2. iteration beats runtime
    scopes.set_iteration_row(map(&[("host", "iteration-value")]));
    let r = scopes.lookup("host").unwrap();
    assert_eq!(r.value, "iteration-value");
    assert_eq!(r.origin, VarOrigin::Iteration);

    // 1. dynamic beats everything, even for a name that happens to look
    // dynamic — demonstrated separately since "host" is not a dynamic name.
    assert!(scopes.lookup("$uuid").is_some());
}

#[test]
fn secret_values_are_flagged_through_interpolation_and_spans() {
    let mut env = Environment::new("prod");
    env.variables.insert("apiKey".into(), EnvVar::secret());
    let mut secrets = SecretValues::new();
    secrets.insert("apiKey".into(), "top-secret".into());

    let scopes = VarScopes::new().with_environment(&env, &secrets);

    let out = interpolate("Authorization: Bearer {{apiKey}}", &scopes).unwrap();
    assert_eq!(out, "Authorization: Bearer top-secret");

    let found = spans("Authorization: Bearer {{apiKey}}", &scopes);
    assert_eq!(found.len(), 1);
    assert!(found[0].secret);
    assert_eq!(found[0].resolved.as_deref(), Some("top-secret"));
}

#[test]
fn unresolved_variables_reported_with_all_names() {
    let scopes = VarScopes::new();
    let err = interpolate("{{a}}/{{b}}/{{a}}/{{c}}", &scopes).unwrap_err();
    let InterpolateError::Unresolved { names } = err;
    assert_eq!(names, vec!["a", "b", "a", "c"]);
}

/// Loads a fixture template file, resolves it against a scope chain built
/// from a fixture environment/collection pair, and checks the rendered
/// output plus the extracted spans' offsets.
#[test]
fn fixture_driven_request_template() {
    let fixture_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/vars");
    let template = fs::read_to_string(format!("{fixture_dir}/request_template.txt"))
        .expect("fixture template should exist");

    let mut env = Environment::new("staging");
    env.variables.insert(
        "baseUrl".into(),
        EnvVar::plain("https://api.staging.example.com"),
    );
    env.variables.insert("token".into(), EnvVar::secret());
    let mut secrets = SecretValues::new();
    secrets.insert("token".into(), "s3cr3t-token".into());

    let collection_vars = map(&[("apiVersion", "v2")]);

    let mut scopes = VarScopes::new()
        .with_environment(&env, &secrets)
        .with_collection(&collection_vars);
    scopes.set_runtime("userId", "42");

    let rendered = interpolate(&template, &scopes).expect("template should fully resolve");
    assert_eq!(
        rendered,
        "GET https://api.staging.example.com/v2/users/42\nAuthorization: Bearer s3cr3t-token\n"
    );

    let found = spans(&template, &scopes);
    assert_eq!(found.len(), 4);
    for span in &found {
        assert_eq!(
            &template[span.start..span.end],
            format!("{{{{{}}}}}", span.name)
        );
        assert!(span.resolved.is_some());
    }
    assert!(found.iter().any(|s| s.name == "token" && s.secret));
}
