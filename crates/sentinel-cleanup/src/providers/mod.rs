//! Built-in cleanup providers.

mod user_temp;

pub use user_temp::UserTemp;

use crate::CleanupProvider;

/// All built-in providers for this machine.
pub fn builtin() -> Vec<Box<dyn CleanupProvider>> {
    vec![Box::new(UserTemp::for_system())]
}
