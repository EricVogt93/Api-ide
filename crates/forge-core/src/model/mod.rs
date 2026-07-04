//! Domain model: the serde types that make up a Forge workspace on disk.
//!
//! Every file-level type carries a `format` field for forward migration.
//! Identity of requests/folders is their file/directory name — these types
//! deliberately contain no UUIDs so that git renames stay clean.

mod assertion;
mod auth;
mod body;
mod collection;
mod environment;
mod extraction;
mod hooks;
mod keyvalue;
mod request;
mod workspace;

pub use assertion::*;
pub use auth::*;
pub use body::*;
pub use collection::*;
pub use environment::*;
pub use extraction::*;
pub use hooks::*;
pub use keyvalue::*;
pub use request::*;
pub use workspace::*;

pub fn default_format() -> u32 {
    crate::FORMAT_VERSION
}

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn is_true(b: &bool) -> bool {
    *b
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}
