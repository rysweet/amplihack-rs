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
fn a_selected_custom_name_has_clean_drift_and_an_unchanged_second_merge() {
    use hook_block::Merge;
    use serde_json::json;

    for bin in [
        "/opt/test/amplihack-hooks-v2",
        "/opt/test/custom-hook-runner",
    ] {
        // Exercise the no-deployed-binary fallback used by the override.
        let selected = binary::choose_durable_hooks_path(None, PathBuf::from(bin));
        for limit_resume in [true, false] {
            let t = HookTarget::new(selected.clone(), limit_resume).unwrap();
            let foreign = json!({"hooks": [
                {"type": "command", "command": format!("\"{bin}-wrapper\" stop")},
                {"type": "command", "command": format!("\"/other{bin}\" stop")},
                {"type": "command", "command": format!("\"{bin}\" --verbose stop")}
            ]});
            let mut settings = json!({"hooks": {"Stop": [foreign.clone()]}});
            assert_eq!(hook_block::merge(&mut settings, &t), Merge::Changed);
            assert!(hook_block::drift(&settings, &t).is_clean());
            assert!(
                settings::validate_amplihack_native_hook_contract(
                    &settings,
                    limit_resume,
                    &selected
                )
                .is_empty()
            );
            let once = settings.clone();
            assert_eq!(hook_block::merge(&mut settings, &t), Merge::Unchanged);
            assert_eq!(settings, once);
            assert_eq!(settings["hooks"]["Stop"][1], foreign);

            // A custom target's obsolete registration is still ours to remove.
            let obsolete = HookTarget::new(selected.clone(), !limit_resume).unwrap();
            hook_block::merge(&mut settings, &obsolete);
            assert!(hook_block::drift(&settings, &obsolete).is_clean());
            assert_eq!(
                hook_block::merge(&mut settings, &obsolete),
                Merge::Unchanged
            );
        }
    }
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

#[test]
fn a_selected_target_takes_precedence_over_legacy_path_detection() {
    for bin in [
        "/opt/test/tools/xpia/hooks/custom-hook-runner",
        "/opt/test/tools/amplihack/hooks/session_start.py",
    ] {
        let t = HookTarget::new(PathBuf::from(bin), true).unwrap();
        let mut settings = serde_json::json!({"hooks": hook_block::block(&t)});
        assert!(hook_block::drift(&settings, &t).is_clean());
        assert_eq!(
            hook_block::merge(&mut settings, &t),
            hook_block::Merge::Unchanged
        );
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
