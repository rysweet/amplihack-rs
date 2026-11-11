//! Issue #2279 (C1, SR-7): `HookTarget::new` validates the hooks-binary path
//! with `validate_binary_path` and returns `None` on an unsafe character, so
//! `build_hook_wrapper`'s `expect()` can never be reached from the rotator
//! daemon or a launch. No panic and no write.

use super::*;
use hook_block::HookTarget;
use std::path::PathBuf;

#[test]
fn a_safe_path_gives_a_target() {
    let t = HookTarget::new(
        PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks"),
        true,
    )
    .expect("a plain absolute path is accepted");
    assert_eq!(
        t.bin,
        PathBuf::from("/opt/amplihack-test/bin/amplihack-hooks")
    );
    assert!(t.limit_resume);
}

#[test]
fn unsafe_characters_give_none_without_panicking() {
    for bad in [
        "/opt/with space/amplihack-hooks",
        "/opt/$HOME/amplihack-hooks",
        "/opt/a;rm/amplihack-hooks",
        "/opt/a`x`/amplihack-hooks",
        "/opt/a\"q/amplihack-hooks",
    ] {
        let result = std::panic::catch_unwind(|| HookTarget::new(PathBuf::from(bad), true));
        let target = result.unwrap_or_else(|_| panic!("panicked on {bad}"));
        assert!(target.is_none(), "{bad} must be refused");
    }
}

#[cfg(unix)]
#[test]
fn an_unsafe_target_never_writes_a_file() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("settings.json");
    fs::write(&path, b"{}\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    // The only way to a HookTarget from a runtime path is HookTarget::new, so
    // an unsafe path never reaches merge_file.
    assert!(HookTarget::new(PathBuf::from("/opt/a b/amplihack-hooks"), true).is_none());
    assert_eq!(fs::read(&path).unwrap(), b"{}\n");
}
