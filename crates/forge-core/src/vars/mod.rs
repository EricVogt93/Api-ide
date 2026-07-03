//! Variable scopes, `{{variable}}` interpolation, built-in dynamic
//! variables and span extraction for editor highlighting.
//!
//! [`VarScopes`] owns the resolution chain (dynamic > iteration > runtime >
//! environment > folder > collection, see its docs for details).
//! [`interpolate`] expands every `{{name}}` reference in a template against
//! a `VarScopes`; [`spans`] extracts the same references without erroring,
//! for editor highlighting/hover.

mod dynamic;
mod interpolate;
mod scope;

pub use interpolate::{interpolate, spans, InterpolateError, VarSpan};
pub use scope::{ResolvedVar, VarOrigin, VarScopes};

/// Resolve a single built-in dynamic variable by name (leading `$`
/// included), re-exported for callers that want to probe dynamic-ness
/// directly rather than going through [`VarScopes::lookup`].
pub fn resolve_dynamic(name: &str) -> Option<String> {
    dynamic::resolve(name)
}
