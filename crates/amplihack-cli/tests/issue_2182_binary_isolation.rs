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
fn path_filter_hides_only_the_resolver_and_keeps_tools_beside_it() {
    use crate::isolated_amplihack::{RESOLVER_BIN, path_without_resolver};
    use std::os::unix::fs::PermissionsExt;
    let shared = tempfile::tempdir().expect("tempdir");
    let tools = tempfile::tempdir().expect("tempdir");
    // A resolver that shares its directory with other tools, as it can in
    // `~/.cargo/bin`, `~/.local/bin` or `/usr/local/bin`.
    for name in [RESOLVER_BIN, "cargo", "sccache"] {
        let file = shared.path().join(name);
        std::fs::write(&file, "#!/bin/sh\nexit 0\n").expect("write tool");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    // A symlink to the resolver is hidden as well.
    let linked = tempfile::tempdir().expect("tempdir");
    std::os::unix::fs::symlink(
        shared.path().join(RESOLVER_BIN),
        linked.path().join(RESOLVER_BIN),
    )
    .expect("symlink resolver");
    std::fs::write(linked.path().join("recipe-runner-rs"), "").expect("write tool");
    let path = std::env::join_paths([tools.path(), shared.path(), linked.path()]).expect("join");

    let filtered = path_without_resolver(&path);

    let dirs: Vec<_> = std::env::split_paths(&filtered).collect();
    assert_eq!(dirs.len(), 3, "no directory is dropped: {dirs:?}");
    assert_eq!(
        dirs[0],
        tools.path(),
        "a directory without a resolver is kept as it is"
    );
    for dir in &dirs {
        assert!(
            dir.join(RESOLVER_BIN).symlink_metadata().is_err(),
            "{} still offers a resolver",
            dir.display()
        );
    }
    for name in ["cargo", "sccache"] {
        let found = dirs[1].join(name);
        assert_eq!(
            std::fs::canonicalize(&found).expect("tool reachable"),
            std::fs::canonicalize(shared.path().join(name)).unwrap(),
            "{name} beside the resolver must stay on PATH"
        );
    }
    assert!(dirs[2].join("recipe-runner-rs").exists());
    // The same directory is shadowed once per process.
    assert_eq!(path_without_resolver(&path), filtered);
}
