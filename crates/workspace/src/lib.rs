//! necoder workspace shell.
//!
//! The public surface stays at the crate root while the implementation is split into
//! feature-owned modules. Keeping this facade small lets callers remain stable during
//! the architecture refactor.

mod persistence;
mod workspace;

pub mod brand_migration;
pub mod crash;
pub mod logging;
pub mod shell_env;
pub mod updater;

pub use brand_migration::migrate_legacy_brand_data;
pub use crash::install_panic_hook;
pub use logging::redirect_output_for_gui_launch;
pub use persistence::{
    decode_window_session, install_window_close_hook, mark_quitting, new_window_session_id,
    open_default_storage, RestoredTabs, SavedProject, WindowPersistence,
};
pub use project::ProjectSource;
pub use shell_env::inherit_login_shell_path;
pub use workspace::*;
