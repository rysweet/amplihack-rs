//! Issue #1551: when the `gh` merge guard applies, and where it is found.

use super::merge_guard::{
    GATE_FILES, GUARD_REL, MERGE_GUARD_ENV, guard_applies, guard_dir_for_home, locate_guard_dir,
};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn constants_name_the_marker_and_the_bundle_dir() {
    assert_eq!(MERGE_GUARD_ENV, "AMPLIHACK_MERGE_GUARD");
    assert_eq!(GUARD_REL, "amplifier-bundle/tools/gh-merge-guard");
}

#[test]
fn guard_applies_to_auto_drive_recipes_by_name_or_stem() {
    let cases: &[(&str, &str)] = &[
        ("auto-drive-to-merge", "/b/recipes/auto-drive-to-merge.yaml"),
        ("autodrive-build", "/b/recipes/autodrive-build.yaml"),
        (
            "autodrive-merge-loop",
            "/b/recipes/autodrive-merge-loop.yaml",
        ),
        // Name only: launched from a file with another stem.
        ("autodrive-crusty-round", "/b/elsewhere/plain.yaml"),
        // Stem only: the file was given another `name:`.
        ("renamed", "/b/recipes/autodrive-merge-round.yaml"),
        ("renamed", "/b/recipes/auto-drive-to-merge.yaml"),
    ];
    for (name, path) in cases {
        assert!(
            guard_applies(name, Path::new(path), None),
            "{name} at {path} must be guarded"
        );
    }
}

#[test]
fn guard_does_not_apply_to_other_recipes() {
    let cases: &[(&str, &str)] = &[
        ("quality-loop", "/b/recipes/quality-loop.yaml"),
        ("default-workflow", "/b/recipes/default-workflow.yaml"),
        ("smart-orchestrator", "/b/recipes/smart-orchestrator.yaml"),
        // Near misses: no hyphen, a prefix inside the name, an extension only.
        ("autodrivex", "/b/recipes/autodrivex.yaml"),
        ("my-autodrive-build", "/b/recipes/my-autodrive-build.yaml"),
        (
            "auto-drive-to-merge-notes",
            "/b/recipes/auto-drive-to-merge-notes.yaml",
        ),
        ("x", "/b/autodrive-dir/x.yaml"),
    ];
    for (name, path) in cases {
        assert!(
            !guard_applies(name, Path::new(path), None),
            "{name} at {path} must not be guarded"
        );
    }
}

/// AZ-1: the marker can only switch the guard on. Values that read as "off"
/// elsewhere ("0", "false", "no", "off") still keep it on.
#[test]
fn any_non_empty_inherited_marker_applies_the_guard() {
    let unrelated = Path::new("/b/recipes/quality-loop.yaml");
    for value in ["1", "yes", "0", "true", "false", "FALSE", "no", "off", " "] {
        assert!(
            guard_applies("quality-loop", unrelated, Some(value)),
            "inherited marker {value:?} must keep the guard on"
        );
    }
}

#[test]
fn an_empty_inherited_marker_does_not_apply_the_guard() {
    let unrelated = Path::new("/b/recipes/quality-loop.yaml");
    assert!(!guard_applies("quality-loop", unrelated, Some("")));
    assert!(!guard_applies("quality-loop", unrelated, None));
}

fn bundle_with_shim(mode: Option<u32>) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let dir = home.join(GUARD_REL);
    fs::create_dir_all(&dir).unwrap();
    // Explicit, so the umask (often 002) cannot make the dir, or tools/
    // above it, group-writable.
    set_mode(dir.parent().unwrap(), 0o755);
    set_mode(&dir, 0o755);
    if let Some(mode) = mode {
        let shim = dir.join("gh");
        fs::write(&shim, "#!/usr/bin/env bash\n# amplihack-gh-merge-guard\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    (tmp, home)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
#[test]
fn locate_guard_dir_returns_the_dir_holding_an_executable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let dir = locate_guard_dir(&home).expect("an executable shim is found");
    assert_eq!(dir, home.join(GUARD_REL));
}

fn assert_refused(home: &Path) {
    let err = locate_guard_dir(home)
        .expect_err("must fail closed")
        .to_string();
    let want = format!(
        "amplihack: merge guard not found at {}; refusing to run auto-drive without it (#1551)",
        home.join(GUARD_REL).display()
    );
    assert_eq!(err, want);
}

#[test]
fn locate_guard_dir_refuses_a_missing_shim() {
    let (_tmp, home) = bundle_with_shim(None);
    assert_refused(&home);
}

#[test]
fn locate_guard_dir_refuses_a_missing_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    assert_refused(tmp.path());
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_non_executable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o644));
    assert_refused(&home);
}

#[test]
fn locate_guard_dir_refuses_a_shim_that_is_a_directory() {
    let (_tmp, home) = bundle_with_shim(None);
    fs::create_dir_all(home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused(&home);
}

/// A symlinked shim is followed for the checks, and the directory is returned
/// as given, never canonicalised, so PATH de-duplication still matches it.
#[cfg(unix)]
#[test]
fn locate_guard_dir_follows_a_symlinked_shim_without_canonicalising() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, home) = bundle_with_shim(None);
    let target = home.join("real-gh");
    fs::write(&target, "#!/bin/bash -p\n# amplihack-gh-merge-guard\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&target, home.join(GUARD_REL).join("gh")).unwrap();
    let dir = locate_guard_dir(&home).expect("a symlink to an executable shim is found");
    assert_eq!(dir, home.join(GUARD_REL));
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_dangling_symlink() {
    let (_tmp, home) = bundle_with_shim(None);
    std::os::unix::fs::symlink(home.join("nowhere"), home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused(&home);
}

/// A relative `AMPLIHACK_HOME` would resolve against the working directory.
#[test]
fn locate_guard_dir_refuses_a_relative_home() {
    let home = Path::new("relative/amplihack-home");
    let err = locate_guard_dir(home)
        .expect_err("a relative home must fail closed")
        .to_string();
    assert_eq!(
        err,
        "amplihack: AMPLIHACK_HOME is not absolute (relative/amplihack-home); refusing to run \
         auto-drive without the merge guard (#1551)"
    );
}

#[test]
fn an_unset_home_is_named_as_the_cause() {
    let err = guard_dir_for_home(None)
        .expect_err("an unset home must fail closed")
        .to_string();
    assert_eq!(
        err,
        "AMPLIHACK_HOME is unset; refusing to run auto-drive without the merge guard (#1551)"
    );
}

/// An empty `AMPLIHACK_HOME` is not absolute either, and is named as such.
#[test]
fn locate_guard_dir_names_an_empty_home_as_not_absolute() {
    let err = locate_guard_dir(Path::new(""))
        .expect_err("an empty home must fail closed")
        .to_string();
    assert!(err.contains("AMPLIHACK_HOME is not absolute"), "{err}");
}

/// G11: a shim or guard directory that group or others can write is not
/// trusted: anyone with that access could replace the guard.
#[cfg(unix)]
fn assert_refused_as_writable(home: &Path, what: &str) {
    let err = locate_guard_dir(home)
        .expect_err(&format!("{what} must fail closed"))
        .to_string();
    assert!(err.contains("(#1551)"), "{what}: {err}");
    assert!(
        err.contains(&home.join(GUARD_REL).display().to_string()),
        "{what}: the error must name the guard dir: {err}"
    );
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_group_writable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o775));
    assert_refused_as_writable(&home, "a 0775 shim");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_an_other_writable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o757));
    assert_refused_as_writable(&home, "a 0757 shim");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_group_writable_guard_dir() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    set_mode(&home.join(GUARD_REL), 0o775);
    assert_refused_as_writable(&home, "a 0775 guard dir");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_an_other_writable_guard_dir() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    // Sticky does not help: others can still add files beside the shim.
    set_mode(&home.join(GUARD_REL), 0o1777);
    assert_refused_as_writable(&home, "a 1777 guard dir");
}

/// A symlinked shim is judged by its target's mode.
#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_symlink_to_a_writable_shim() {
    let (_tmp, home) = bundle_with_shim(None);
    let target = home.join("real-gh");
    fs::write(&target, "#!/bin/bash -p\n# amplihack-gh-merge-guard\n").unwrap();
    set_mode(&target, 0o777);
    std::os::unix::fs::symlink(&target, home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused_as_writable(&home, "a symlink to a 0777 shim");
}

/// Owner-only modes are fine: 0700 and 0500 are as safe as 0755.
#[cfg(unix)]
#[test]
fn locate_guard_dir_accepts_owner_only_modes() {
    for (shim_mode, dir_mode) in [(0o700, 0o700), (0o500, 0o755), (0o755, 0o555)] {
        let (_tmp, home) = bundle_with_shim(Some(shim_mode));
        set_mode(&home.join(GUARD_REL), dir_mode);
        let found = locate_guard_dir(&home);
        // Restore so the tempdir can be removed.
        set_mode(&home.join(GUARD_REL), 0o755);
        assert_eq!(
            found.expect("owner-only modes are accepted"),
            home.join(GUARD_REL),
            "shim {shim_mode:o}, dir {dir_mode:o}"
        );
    }
}

/// G11: the shim lets the gate merge, so a writable tools/ directory or gate
/// file is as untrustworthy as a writable shim.
#[cfg(unix)]
fn assert_refused_naming(home: &Path, path: &Path, what: &str) {
    let err = locate_guard_dir(home)
        .expect_err(&format!("{what} must fail closed"))
        .to_string();
    assert!(err.contains("(#1551)"), "{what}: {err}");
    assert!(
        err.contains(&path.display().to_string()),
        "{what}: the error must name {}: {err}",
        path.display()
    );
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_group_writable_tools_dir() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let tools = home.join(GUARD_REL).parent().unwrap().to_path_buf();
    set_mode(&tools, 0o775);
    assert_refused_naming(&home, &tools, "a 0775 tools dir");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_writable_gate_or_gate_helper() {
    for name in GATE_FILES {
        for mode in [0o775, 0o757] {
            let (_tmp, home) = bundle_with_shim(Some(0o755));
            let file = home.join(GUARD_REL).parent().unwrap().join(name);
            fs::write(&file, "#!/bin/bash\n").unwrap();
            set_mode(&file, mode);
            assert_refused_naming(&home, &file, &format!("a {mode:o} {name}"));
        }
    }
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_accepts_read_only_gate_files_and_ignores_absent_ones() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let gate = home.join(GUARD_REL).parent().unwrap().join(GATE_FILES[0]);
    fs::write(&gate, "#!/bin/bash\n").unwrap();
    set_mode(&gate, 0o755);
    locate_guard_dir(&home).expect("a 0755 gate beside an absent helper is trusted");
}

/// Issue #1551 D2: the guard is switched on by recipe name, so renaming an
/// auto-drive recipe would silently switch it off. Every shipped recipe that
/// is part of auto-drive (the entry recipe and each `autodrive-*.yaml`) must
/// still be matched by its `name:`, and so must every recipe that runs the
/// merge gate.
#[test]
fn every_shipped_auto_drive_recipe_is_guarded_by_its_name() {
    let recipes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/recipes");
    let mut checked = 0;
    for entry in fs::read_dir(&recipes).expect("the bundle's recipes dir") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let body = fs::read_to_string(&path).unwrap();
        let in_family = stem == "auto-drive-to-merge" || stem.starts_with("autodrive-");
        if !in_family && !body.contains("autodrive_merge_gate.sh") {
            continue;
        }
        let name = body
            .lines()
            .find_map(|l| l.strip_prefix("name:"))
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
            .unwrap_or_else(|| panic!("{} has no top-level name:", path.display()));
        assert!(
            guard_applies(&name, Path::new("unrelated.yaml"), None),
            "{}: name {name:?} no longer switches the #1551 merge guard on",
            path.display()
        );
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected the auto-drive recipes under {}",
        recipes.display()
    );
}

/// Issue #1551: a helper the gate sources runs inside the process the shim
/// trusts to merge, so a sourced helper missing from [`GATE_FILES`] would
/// escape the mode check. Every `autodrive_*.sh` a shipped gate file sources
/// (`. "${GATE_HOME}/…"` or a `shellcheck source=` directive) must be listed.
#[test]
fn every_helper_the_gate_sources_is_a_gate_file() {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools");
    let mut sourced = Vec::new();
    for name in GATE_FILES {
        let text = fs::read_to_string(tools.join(name)).expect("a shipped gate file");
        for marker in [". \"${GATE_HOME}/", "shellcheck source="] {
            for (at, _) in text.match_indices(marker) {
                let rest = &text[at + marker.len()..];
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                    .unwrap_or(rest.len());
                let file = &rest[..end];
                if file.starts_with("autodrive_") && file.ends_with(".sh") {
                    sourced.push(file.to_string());
                }
            }
        }
    }
    assert!(
        sourced.iter().any(|f| f == "autodrive_unmerged.sh"),
        "the scan must see autodrive_state.sh source autodrive_unmerged.sh; found {sourced:?}"
    );
    for file in &sourced {
        assert!(
            GATE_FILES.contains(&file.as_str()),
            "{file} is sourced by the merge gate but missing from GATE_FILES"
        );
    }
}

/// Issue #1551: `amplihack claude` prepends the claude binary's directory,
/// which may also hold the real gh. Inside a guarded run the guard must stay
/// at PATH index 0 for the agent; outside one, PATH is left alone.
#[cfg(unix)]
#[test]
fn keep_guard_first_puts_the_guard_ahead_of_the_agent_binary_dir() {
    use super::merge_guard::keep_guard_first;
    use crate::env_builder::EnvBuilder;
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let home_str = home.to_str().unwrap();
    let launcher = || {
        EnvBuilder::new()
            .set("AMPLIHACK_HOME", home_str)
            .prepend_path("/opt/claude-and-gh/bin")
    };

    let env = keep_guard_first(launcher(), Some("1")).unwrap().build();
    let path = env.get("PATH").unwrap();
    let first = path.split(':').next().unwrap();
    assert_eq!(Path::new(first), home.join(GUARD_REL), "PATH: {path}");
    assert_eq!(env.get(MERGE_GUARD_ENV).map(String::as_str), Some("1"));

    for inherited in [None, Some("")] {
        let env = keep_guard_first(launcher(), inherited).unwrap().build();
        let path = env.get("PATH").unwrap();
        assert!(path.starts_with("/opt/claude-and-gh/bin"), "PATH: {path}");
    }
}

#[test]
fn keep_guard_first_fails_closed_without_a_guard() {
    use super::merge_guard::keep_guard_first;
    use crate::env_builder::EnvBuilder;
    let Err(err) = keep_guard_first(EnvBuilder::new().unset("AMPLIHACK_HOME"), Some("1")) else {
        panic!("a guarded launch with no AMPLIHACK_HOME is refused");
    };
    assert!(err.to_string().contains("#1551"), "{err}");
}
