//! Subprocess fixture contracts: inspect the actual child command without
//! executing an unprotected CLI or staging any host binary in the red phase.
#![cfg(unix)]

use std::ffi::OsStr;
use std::path::Path;

#[test]
fn subprocess_fixture_keeps_all_application_roots_inside_the_private_home() {
    let command = crate::isolated_amplihack::isolated("amplihack");
    let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
    let home = vars
        .get(OsStr::new("HOME"))
        .and_then(|value| *value)
        .expect("fixture must explicitly set HOME");
    // The fixture clears these (issues #1699 and #2922), so the binary derives
    // each root from the private HOME. A value that is passed must lie inside it.
    for key in [
        "AMPLIHACK_HOME",
        "CLAUDE_CONFIG_DIR",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    ] {
        if let Some(root) = vars.get(OsStr::new(key)).and_then(|value| *value) {
            assert!(
                Path::new(root).starts_with(Path::new(home)),
                "{key} must be inside the private HOME"
            );
        }
    }
}

#[test]
fn subprocess_fixture_path_holds_no_resolver_directory() {
    let command = crate::isolated_amplihack::isolated("amplihack");
    let vars: std::collections::HashMap<_, _> = command.get_envs().collect();
    let path = vars
        .get(OsStr::new("PATH"))
        .and_then(|value| *value)
        .expect("fixture must explicitly control PATH");
    for dir in std::env::split_paths(path) {
        assert!(
            !dir.join(crate::isolated_amplihack::RESOLVER_BIN).exists(),
            "{} would let the child discover a host resolver",
            dir.display()
        );
    }
}

#[test]
fn path_filter_drops_resolver_directories_and_keeps_the_rest() {
    let with_resolver = tempfile::tempdir().expect("tempdir");
    let tools = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        with_resolver
            .path()
            .join(crate::isolated_amplihack::RESOLVER_BIN),
        "",
    )
    .expect("write resolver");
    let path = std::env::join_paths([tools.path(), with_resolver.path()]).expect("join");

    let filtered = crate::isolated_amplihack::path_without_resolver(&path);

    let dirs: Vec<_> = std::env::split_paths(&filtered).collect();
    assert_eq!(dirs, vec![tools.path().to_path_buf()]);
}
