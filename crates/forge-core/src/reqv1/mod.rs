//! Request format v1: a local-first, Git-friendly, deterministic request
//! format and execution engine. One request per JSON file, all reuse by
//! reference into an asset store, all behavior as ordered pipeline assets.
//!
//! See `docs/architecture/request-format-v1.md` for the full specification
//! and `schemas/request-v1.schema.json` for the document schema.
//!
//! Stage flow (§12): parse → refs → bindings → variables → canonical IR →
//! pipeline → HTTP/mock → result. Stages up to the IR are pure (no network)
//! and back [`validate`]; running adds the HTTP send.

pub mod assertions;
pub mod build;
pub mod bundle;
pub mod catalog;
pub mod diag;
pub mod environment_scope;
pub mod hooks;
pub mod index;
pub mod ir;
pub mod jshost;
pub mod lock;
pub mod matrix;
pub mod migrate;
pub mod mock;
pub mod model;
pub mod openapi_generate;
pub mod openapi_scope;
pub mod pipeline;
pub mod refs;
pub mod resolve;
pub mod runner;
pub mod scaffold;
pub mod secrets;
pub mod sequence;
pub mod tickets;
pub mod vars;

pub use assertions::{
    assertions_path, load_request_document, AssertionDocument, AssertionEntry, AssertionKind,
};
pub use build::{build_ir, BuildInputs};
pub use bundle::{export_bundle, import_bundle, BundleFormat, ExportSummary, ImportSummary};
pub use catalog::{
    builtin_catalog, find_builtin, BuiltinDefinition, BuiltinIntent, BuiltinParameter,
    BuiltinParameterKind, BuiltinTarget, ProjectAssetMetadata, ProjectAssetParameter,
};
pub use diag::{Code, Diagnostic, Errors, Severity};
pub use environment_scope::{
    effective_environment, own_environment, remove_environment, set_environment,
    EnvironmentSelection,
};
pub use hooks::{hooks_path, HookDocument, HookKind};
pub use index::{AssetEntry, AssetKind, ProjectIndex};
pub use ir::{ResolvedBody, ResolvedHeader, ResolvedRequest};
pub use lock::Lockfile;
pub use matrix::{
    run_matrix, run_matrix_with_responses, run_matrix_with_responses_in_session, MatrixCase,
};
pub use migrate::{migrate_request, migrate_tree, MigrationError, MigrationItem, MigrationStatus};
pub use mock::{MockRoute, MockServerConfig};
pub use model::{Binding, ProjectAuthConfig, ProjectConfig, RequestDocument};
pub use openapi_generate::{generate_openapi_suite, GeneratedOpenApiSuite, OpenApiSuiteKind};
pub use openapi_scope::{
    effective_openapi, own_openapi, remove_openapi, set_openapi, OpenApiSelection,
};
pub use pipeline::{
    run_after_response, run_before_request, AssertionResult, RequestPatch, ResponseView,
};
pub use refs::{AssetDescriptor, RefResolver, RefScheme};
pub use resolve::DataStore;
pub use runner::{
    load_environment, load_project, load_project_auth_document, load_request_environment, run,
    run_sequence, run_sequence_with_environment_values_in_session, run_sequence_with_responses,
    run_sequence_with_responses_in_session, run_with_response, run_with_response_in_session,
    run_with_runtime, validate, validate_case, AuthSession, HttpResultView, RunMode, RunResult,
    RunStatus,
};
pub use scaffold::{available_path, scaffold_asset, ScaffoldedAsset};
pub use secrets::load_file_secrets;
pub use sequence::{SequenceDocument, SequenceKind};
pub use tickets::{
    effective_ticket, own_ticket, remove_ticket, set_ticket, ticket_label, TicketLink,
};
