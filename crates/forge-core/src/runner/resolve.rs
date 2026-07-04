//! Turns a stored [`RequestDef`] plus variable scopes into a
//! [`ResolvedRequest`] ready for the HTTP engine: interpolation, query/path
//! params, headers, auth (including OAuth2 client-credentials token fetch)
//! and body materialization.

use crate::exec::{ExecError, HttpEngine, ResolvedRequest};
use crate::model::RequestDef;
use crate::store::Workspace;
use crate::vars::{InterpolateError, VarScopes};

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Interpolate(#[from] InterpolateError),
    #[error("auth: {0}")]
    Auth(String),
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error("invalid body: {0}")]
    Body(String),
}

/// Auth configs collected from the request's ancestor chain, outermost last;
/// used to resolve `AuthConfig::Inherit`.
pub type AuthChain<'a> = Vec<&'a crate::model::AuthConfig>;

/// Resolve `def` into an executable request.
///
/// `engine` is needed for OAuth2 client-credentials token fetches (cached).
/// `workspace` provides root-relative body file paths and settings defaults.
pub async fn resolve_request(
    workspace: &Workspace,
    def: &RequestDef,
    auth_chain: &AuthChain<'_>,
    scopes: &VarScopes,
    engine: &HttpEngine,
) -> Result<ResolvedRequest, ResolveError> {
    let _ = (workspace, def, auth_chain, scopes, engine);
    todo!("implemented by the runner module (wave B)")
}
