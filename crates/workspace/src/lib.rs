//! Shirushi workspace shell.
//!
//! The public surface stays at the crate root while the implementation is split into
//! feature-owned modules. Keeping this facade small lets callers remain stable during
//! the architecture refactor.

mod persistence;
mod workspace;

pub mod crash;
pub mod updater;

pub use crash::install_panic_hook;
pub use persistence::{RestoredTabs, SavedProject, load_saved_state, load_state, state_path};
pub use project::ProjectSource;
pub use workspace::*;
