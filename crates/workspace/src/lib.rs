//! Shirushi workspace shell.
//!
//! The public surface stays at the crate root while the implementation is split into
//! feature-owned modules. Keeping this facade small lets callers remain stable during
//! the architecture refactor.

mod persistence;
mod workspace;

pub mod crash;
pub mod logging;
pub mod shell_env;
pub mod updater;

pub use crash::install_panic_hook;
pub use logging::redirect_output_for_gui_launch;
pub use persistence::{load_saved_state, load_state, state_path, RestoredTabs, SavedProject};
pub use project::ProjectSource;
pub use shell_env::inherit_login_shell_path;
pub use workspace::*;
