//! "Start Sentinel when I sign in": one value in the current user's Run key.
//!
//! Per-user (`HKEY_CURRENT_USER`), so it needs no administrator rights and affects no other
//! account. It is only ever written when the person turns the setting on, holds a single quoted
//! command (the path of this program and `--background`), and is removed when they turn it
//! off. The registry is the source of truth for whether it is on: a person can also remove it
//! in Windows' own Startup apps list, and the setting then shows as off.

use std::path::Path;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Sentinel";
/// The argument that starts Sentinel hidden in the notification area.
pub(crate) const BACKGROUND_ARG: &str = "--background";

/// The command stored for `exe`. The path is quoted, so a folder with spaces cannot make Windows
/// run something shorter than intended.
pub(crate) fn command_for(exe: &Path) -> Result<String, String> {
    let text = exe.to_str().ok_or("the program path is not valid text")?;
    if text.contains('"') || !exe.is_absolute() {
        return Err("the program path is not a plain absolute path".into());
    }
    Ok(format!("\"{text}\" {BACKGROUND_ARG}"))
}

fn open(key_path: &str, access: u32) -> std::io::Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(key_path, access)
}

/// Whether `key_path` holds exactly the command for `exe`.
pub(crate) fn is_enabled_in(key_path: &str, exe: &Path) -> bool {
    let Ok(want) = command_for(exe) else {
        return false;
    };
    open(key_path, KEY_READ)
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
        .is_ok_and(|v| v.eq_ignore_ascii_case(&want))
}

pub(crate) fn enable_in(key_path: &str, exe: &Path) -> Result<(), String> {
    let command = command_for(exe)?;
    RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(key_path)
        .and_then(|(k, _)| k.set_value(VALUE_NAME, &command))
        .map_err(|e| format!("Windows would not save the startup entry: {e}"))
}

pub(crate) fn disable_in(key_path: &str) -> Result<(), String> {
    match open(key_path, KEY_SET_VALUE).and_then(|k| k.delete_value(VALUE_NAME)) {
        Ok(()) => Ok(()),
        // Nothing to remove is the state that was asked for.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Windows would not remove the startup entry: {e}")),
    }
}

fn current_exe() -> Result<std::path::PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("could not find this program: {e}"))
}

pub(crate) fn is_enabled() -> bool {
    current_exe().is_ok_and(|exe| is_enabled_in(RUN_KEY, &exe))
}

/// Turn the startup entry on or off for this copy of Sentinel.
pub(crate) fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        enable_in(RUN_KEY, &current_exe()?)
    } else {
        disable_in(RUN_KEY)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use super::*;

    /// A throwaway key under HKCU, removed afterwards; never the real Run key.
    struct TestKey(String);

    impl TestKey {
        fn new() -> Self {
            Self(format!(
                r"Software\SentinelTests\Run-{}",
                uuid::Uuid::new_v4()
            ))
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(&self.0);
        }
    }

    #[test]
    fn the_command_is_one_quoted_path_and_the_background_flag() {
        let exe = PathBuf::from(r"C:\Program Files\Sentinel\Sentinel.exe");
        assert_eq!(
            command_for(&exe).unwrap(),
            r#""C:\Program Files\Sentinel\Sentinel.exe" --background"#
        );
        // Anything that could change what runs is refused rather than escaped.
        assert!(command_for(Path::new(r#"C:\a"b\x.exe"#)).is_err());
        assert!(command_for(Path::new(r"relative\x.exe")).is_err());
    }

    #[test]
    fn it_is_off_until_turned_on_and_off_again_after_turning_off() {
        let key = TestKey::new();
        let exe = PathBuf::from(r"C:\Tools\Sentinel\Sentinel.exe");
        assert!(
            !is_enabled_in(&key.0, &exe),
            "nothing is written by default"
        );
        enable_in(&key.0, &exe).unwrap();
        assert!(is_enabled_in(&key.0, &exe));
        enable_in(&key.0, &exe).unwrap(); // idempotent
        disable_in(&key.0).unwrap();
        assert!(!is_enabled_in(&key.0, &exe));
        disable_in(&key.0).unwrap(); // removing what is not there is fine
    }

    #[test]
    fn an_entry_for_a_different_copy_does_not_count_as_this_one() {
        let key = TestKey::new();
        let a = PathBuf::from(r"C:\Tools\A\Sentinel.exe");
        let b = PathBuf::from(r"C:\Tools\B\Sentinel.exe");
        enable_in(&key.0, &a).unwrap();
        assert!(is_enabled_in(&key.0, &a));
        assert!(!is_enabled_in(&key.0, &b));
        // Enabling from the other copy replaces it: there is only ever one entry.
        enable_in(&key.0, &b).unwrap();
        assert!(is_enabled_in(&key.0, &b) && !is_enabled_in(&key.0, &a));
    }
}
