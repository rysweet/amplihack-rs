//! Issue #2528: `recipe run` selects one installed bundle and refuses a run
//! with none (E4). Fixtures whose recipes are given as paths need only an
//! empty, trusted bundle at the home they name.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Writes `<home>/amplifier-bundle` with empty `recipes/` and `agents/`
/// folders, mode 0755, whatever the umask. Safe to call more than once.
pub fn ensure(home: &Path) -> PathBuf {
    let bundle = home.join("amplifier-bundle");
    for dir in ["recipes", "agents"] {
        fs::create_dir_all(bundle.join(dir)).unwrap();
    }
    for dir in [
        bundle.join("recipes"),
        bundle.join("agents"),
        bundle.clone(),
    ] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    bundle
}

/// [`ensure`] at `home`, returning `home` for `.env("AMPLIHACK_HOME", ...)`.
pub fn home(home: impl AsRef<Path>) -> PathBuf {
    ensure(home.as_ref());
    home.as_ref().to_path_buf()
}
