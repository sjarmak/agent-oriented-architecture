//! The path-trust boundary: resolving a caller-supplied path safely under a
//! trust root.
//!
//! One crate owns this invariant so a bypass fixed here is fixed for every
//! consumer. Three questions live here, and they are deliberately distinct:
//!
//! - [`validate_single_component`] — may this *name* be joined onto a trusted
//!   base at all?
//! - [`safe_join_nofollow`] — walk a relative path under a root, refusing every
//!   existing symlink. Check-then-act; the portable fallback.
//! - [`dirfd`] (Unix) — acquire each directory with
//!   `openat(O_NOFOLLOW | O_DIRECTORY)` and mutate relative to the descriptor.
//!   This, not the walker, is the authorization check where it is available:
//!   a path substituted after a check cannot redirect a descriptor-relative
//!   write.
//! - [`resolve_canonicalizing`] — resolve a path whose leaf may not exist yet to
//!   the destination a write would reach, leaving containment to the caller.
//! - [`resolve_repository_root`] — which root the other three are relative to.
//!   A host that gets the containment checks would otherwise re-derive the
//!   anchor itself, and an anchor derived twice is two answers.

mod component;
#[cfg(unix)]
pub mod dirfd;
mod error;
mod nofollow;
mod resolve;
mod root;

pub use component::validate_single_component;
pub use error::{PathTrustError, UnsafePathComponent};
pub use nofollow::{is_symlink_nofollow, reject_symlink, safe_join_nofollow};
pub use resolve::{normalize_lexically, resolve_canonicalizing};
pub use root::{resolve_repository_root, RepositoryRootError};
