//! Issue #1551: when the `gh` merge guard applies, and where it is found.

use super::merge_guard::{
    BASH_ENV_FILE, GATE_FILES, GUARD_REL, MERGE_GUARD_ENV, guard_applies, guard_dir_for_home,
    locate_guard_dir,
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
    fs::write(dir.join(BASH_ENV_FILE), "# bash-env\n").unwrap();
    set_mode(&dir.join(BASH_ENV_FILE), 0o644);
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

/// Issue #1763: `autodrive_trust.sh` loads `autodrive_qa_scope.sh` from its
/// own directory, so the scope helper runs inside the gate process and needs
/// the shim's mode check, and it must be shipped.
#[test]
fn the_qa_scope_helper_is_a_gate_file() {
    assert!(
        GATE_FILES.contains(&"autodrive_qa_scope.sh"),
        "autodrive_qa_scope.sh is loaded by autodrive_trust.sh but missing from GATE_FILES: {GATE_FILES:?}"
    );
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools");
    for name in GATE_FILES {
        assert!(
            tools.join(name).is_file(),
            "GATE_FILES names {name}, which is not shipped in amplifier-bundle/tools"
        );
    }
    let trust = fs::read_to_string(tools.join("autodrive_trust.sh")).expect("autodrive_trust.sh");
    assert!(
        trust.contains("shellcheck source=autodrive_qa_scope.sh"),
        "autodrive_trust.sh must name autodrive_qa_scope.sh in a shellcheck source= directive \
         so every_helper_the_gate_sources_is_a_gate_file sees it"
    );
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
    assert_eq!(
        env.get("BASH_ENV").map(PathBuf::from),
        Some(home.join(GUARD_REL).join(BASH_ENV_FILE)),
        "the agent's shells re-assert the guard after their profile"
    );

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

/// An executable stub file at `path`, for a host-check tool that must exist.
#[cfg(unix)]
fn tool_stub(path: &Path) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    set_mode(path, 0o755);
    path.to_path_buf()
}

/// Crusty round 1 (#1644): a host whose shim reads its parent through /proc,
/// but that has no /proc, used to run all three phases and fail only when the
/// shim refused the gate's own merge. The launch check stops it before any
/// step instead.
///
/// #1646: this arm now fires only on a Unix host that is neither Linux nor
/// Darwin, so the remedy it offers names both platforms that do work. Both
/// `ParentCheck` arms are reachable on every platform, so this runs on Linux
/// CI and on the Mac.
#[cfg(unix)]
#[test]
fn a_proc_parent_check_without_proc_is_refused_at_launch_not_at_the_merge() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    let no_proc = tmp.path().join("proc/self/stat");
    let err = check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Proc)
        .expect_err("no /proc must stop the run")
        .to_string();
    assert!(err.contains(&no_proc.display().to_string()), "{err}");
    // F6: the retained "the BSDs" substring is what stops the bail drifting
    // back to claiming macOS is the host that has no /proc. The remedy
    // sentence names macOS deliberately; the diagnosis must not.
    assert!(
        err.contains("the BSDs have no /proc"),
        "the bail must name who has no /proc: {err}"
    );
    assert!(err.contains("#1551"), "{err}");
    assert!(
        err.contains("Run auto-drive on Linux or macOS."),
        "#1646: macOS no longer needs /proc, so it is now a platform this bail \
         sends the operator to, not one it blames: {err}"
    );
}

/// #1646: the macOS arm needs `ps` and `lsof`, not `/proc`. A missing one is
/// named, with the issue number, and the bail never mentions `/proc` — a
/// host-check message that named a path the arm does not use would send an
/// operator looking in the wrong place.
#[cfg(unix)]
#[test]
fn a_tools_parent_check_names_a_missing_tool_and_1646() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately absent: the Tools arm must never look at it.
    let no_proc = tmp.path().join("proc/self/stat");
    let present = tool_stub(&tmp.path().join("bin/ps"));
    let missing = tmp.path().join("usr/sbin/lsof");

    for (tools, named) in [
        (vec![missing.as_path(), present.as_path()], &missing),
        (vec![present.as_path(), missing.as_path()], &missing),
    ] {
        let err = check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Tools(&tools))
            .expect_err("a missing macOS tool must stop the run")
            .to_string();
        assert!(
            err.contains(&named.display().to_string()),
            "the bail must name the missing tool: {err}"
        );
        assert!(err.contains("#1646"), "{err}");
        assert!(
            !err.contains("/proc"),
            "the Tools arm must not blame /proc: {err}"
        );
    }
}

/// A tool that exists but is not executable is refused the same way: "could
/// not tell" must be indistinguishable from "absent" (AZ-1).
#[cfg(unix)]
#[test]
fn a_tools_parent_check_refuses_a_non_executable_tool() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    let no_proc = tmp.path().join("proc/self/stat");
    let ps = tool_stub(&tmp.path().join("bin/ps"));
    let lsof = tool_stub(&tmp.path().join("usr/sbin/lsof"));
    set_mode(&lsof, 0o644);
    let tools = [ps.as_path(), lsof.as_path()];
    let err = check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Tools(&tools))
        .expect_err("a non-executable tool must stop the run")
        .to_string();
    assert!(err.contains(&lsof.display().to_string()), "{err}");
    assert!(err.contains("#1646"), "{err}");

    // A directory where a tool should be is refused too.
    fs::remove_file(&lsof).unwrap();
    fs::create_dir_all(&lsof).unwrap();
    assert!(
        check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Tools(&tools)).is_err(),
        "a directory in place of a tool must stop the run"
    );
}

/// The point of the enum: with the Tools arm, a host with no `/proc` at all
/// passes. This is what lets auto-drive start on macOS.
#[cfg(unix)]
#[test]
fn a_tools_parent_check_passes_without_any_proc() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    let no_proc = tmp.path().join("proc/self/stat");
    let ps = tool_stub(&tmp.path().join("bin/ps"));
    let lsof = tool_stub(&tmp.path().join("usr/sbin/lsof"));
    let tools = [ps.as_path(), lsof.as_path()];
    check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Tools(&tools))
        .expect("ps and lsof present: no /proc is needed");
}

/// #1646 (SEC-A5): an empty tool list refuses. No caller builds one — the
/// macOS arm passes the two pinned constants — so this changes no shipped
/// behaviour. It is here because a check whose empty case succeeds
/// vacuously is the shape a later refactor turns into a bypass, and this
/// one sits on the path to a merge.
#[cfg(unix)]
#[test]
fn a_tools_parent_check_refuses_an_empty_tool_list() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    let no_proc = tmp.path().join("proc/self/stat");
    let err = check_guard_host(&no_proc, Path::new("/bin/sh"), ParentCheck::Tools(&[]))
        .expect_err("an empty tool list must not pass vacuously")
        .to_string();
    assert!(err.contains("#1646"), "{err}");
    assert!(
        !err.contains("/proc"),
        "the Tools arm must not blame /proc: {err}"
    );

    // The interpreter is checked after the parent arm, so an empty list is
    // refused even on a host that has everything else.
    assert!(
        check_guard_host(
            &no_proc,
            Path::new(super::merge_guard::SHIM_INTERPRETER),
            ParentCheck::Tools(&[])
        )
        .is_err(),
        "an empty tool list must refuse regardless of the interpreter"
    );
}

/// The reference doc quotes both launch-check bails verbatim, in fenced
/// `text` blocks, as what an operator will see. An operator who greps the
/// message they got against the doc has to land on the right page, so the
/// two must not drift apart — and the drift this guards against is real:
/// the doc said "Run auto-drive on Linux or macOS." while the code still
/// said "Run auto-drive on Linux." (#1646).
///
/// The comparison is verbatim apart from the path, which the test cannot
/// make missing on the host it runs on: `/proc/self/stat` exists on Linux
/// CI and `/usr/sbin/lsof` exists on this Mac. Each message is produced
/// with a temporary path and the path alone is substituted back, so every
/// other byte — wording, issue number, trailing stop — is compared as
/// written.
#[cfg(unix)]
#[test]
fn the_reference_doc_quotes_both_launch_bails_verbatim() {
    use super::merge_guard::{LSOF_TOOL, PROC_SELF_STAT, ParentCheck, check_guard_host};
    let doc =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/auto-drive-to-merge.md");
    let text = fs::read_to_string(&doc).expect("the auto-drive reference doc");

    let tmp = tempfile::tempdir().unwrap();
    let absent = tmp.path().join("absent");
    let shown = absent.display().to_string();

    // Both arms, produced against a path that really is missing, then with
    // that path swapped for the one the doc quotes.
    let proc_bail = check_guard_host(&absent, Path::new("/bin/sh"), ParentCheck::Proc)
        .expect_err("a missing proc_stat must bail")
        .to_string()
        .replace(&shown, PROC_SELF_STAT);
    let tools = [absent.as_path()];
    let tools_bail = check_guard_host(
        Path::new(PROC_SELF_STAT),
        Path::new("/bin/sh"),
        ParentCheck::Tools(&tools),
    )
    .expect_err("a missing tool must bail")
    .to_string()
    .replace(&shown, LSOF_TOOL);

    for produced in [&proc_bail, &tools_bail] {
        assert!(
            text.lines().any(|line| line == produced),
            "the reference doc does not quote this bail verbatim; it must, or an \
             operator who greps the message they got finds nothing:\n  produced: {produced}"
        );
    }

    // And the other way: the doc must not quote a launch bail the code can
    // no longer produce. A stale quote is worse than a missing one, because
    // it reads as current.
    for line in text.lines() {
        if line.starts_with("amplihack: auto-drive needs ") {
            assert!(
                line == proc_bail || line == tools_bail,
                "the doc quotes a launch bail the code does not produce:\n  doc: {line}"
            );
        }
    }
}

/// No /bin/bash (NixOS): the shim cannot start, so every gh call would fail,
/// not only merges. This bail is for every Unix host, so it must fire under
/// both arms, and it must not be reported as a /proc or a tool problem.
#[cfg(unix)]
#[test]
fn a_host_without_the_shim_interpreter_is_refused_at_launch() {
    use super::merge_guard::{ParentCheck, check_guard_host};
    let tmp = tempfile::tempdir().unwrap();
    let stat = tmp.path().join("stat");
    fs::write(&stat, "1 (x) S 0").unwrap();
    let ps = tool_stub(&tmp.path().join("bin/ps"));
    let lsof = tool_stub(&tmp.path().join("usr/sbin/lsof"));
    let tools = [ps.as_path(), lsof.as_path()];
    let no_bash = tmp.path().join("bin/bash");

    for parent in [ParentCheck::Proc, ParentCheck::Tools(&tools)] {
        let err = check_guard_host(&stat, &no_bash, parent)
            .expect_err("no interpreter must stop the run")
            .to_string();
        assert!(err.contains(&no_bash.display().to_string()), "{err}");
        assert!(err.contains("every gh call"), "{err}");
    }

    // Present but not executable is refused the same way.
    fs::create_dir_all(no_bash.parent().unwrap()).unwrap();
    fs::write(&no_bash, "").unwrap();
    set_mode(&no_bash, 0o644);
    assert!(check_guard_host(&stat, &no_bash, ParentCheck::Proc).is_err());
    assert!(check_guard_host(&stat, &no_bash, ParentCheck::Tools(&tools)).is_err());

    set_mode(&no_bash, 0o755);
    check_guard_host(&stat, &no_bash, ParentCheck::Proc).expect("both present: the host is usable");
    check_guard_host(&stat, &no_bash, ParentCheck::Tools(&tools))
        .expect("both present: the host is usable");
}

/// The real launch path runs the host check: on this (Linux) test host the
/// real /proc/self/stat and /bin/bash exist, and locate_guard_dir passes.
#[cfg(target_os = "linux")]
#[test]
fn locate_guard_dir_checks_the_real_host() {
    use super::merge_guard::{PROC_SELF_STAT, ParentCheck, SHIM_INTERPRETER, check_guard_host};
    assert_eq!(PROC_SELF_STAT, "/proc/self/stat");
    assert_eq!(SHIM_INTERPRETER, "/bin/bash");
    if Path::new(SHIM_INTERPRETER).exists() {
        check_guard_host(
            Path::new(PROC_SELF_STAT),
            Path::new(SHIM_INTERPRETER),
            ParentCheck::Proc,
        )
        .unwrap();
        let (_tmp, home) = bundle_with_shim(Some(0o755));
        locate_guard_dir(&home).expect("a Linux host passes the launch check");
    }
}

/// #1646, the macOS twin: the real launch path asks for `/bin/ps` and
/// `/usr/sbin/lsof` — the two tools the shim's Darwin arm resolves through
/// `sys_tool` — and not for `/proc`, and `locate_guard_dir` passes on this
/// Darwin host. Before #1646 this failed, because the launch check demanded
/// `/proc/self/stat` on every Unix host.
#[cfg(target_os = "macos")]
#[test]
fn locate_guard_dir_checks_the_real_macos_host() {
    use super::merge_guard::{LSOF_TOOL, PS_TOOL, ParentCheck, SHIM_INTERPRETER, check_guard_host};
    assert_eq!(PS_TOOL, "/bin/ps");
    assert_eq!(LSOF_TOOL, "/usr/sbin/lsof");
    assert_eq!(SHIM_INTERPRETER, "/bin/bash");
    let tools = [Path::new(PS_TOOL), Path::new(LSOF_TOOL)];
    check_guard_host(
        Path::new("/nonexistent/proc/self/stat"),
        Path::new(SHIM_INTERPRETER),
        ParentCheck::Tools(&tools),
    )
    .expect("this macOS host has ps, lsof and /bin/bash");

    let (_tmp, home) = bundle_with_shim(Some(0o755));
    locate_guard_dir(&home).expect("#1646: macOS passes the launch check without /proc");
}

/// #1646 (SEC-A8): the interpreter is pinned in three places that must agree
/// — the shim's `#!` line, its `SHIM_BASH` constant, and `SHIM_INTERPRETER`
/// here. On macOS that one path carries the whole fact (e) argument: both
/// that dyld strips `DYLD_*` before the parent starts, and that `ps -E`
/// withholds the parent's environment so the shim reads its own inherited
/// one instead, hold because `/bin/bash` is a SIP-protected platform binary.
/// Repointing any of the three at a bash outside a SIP-protected root — a
/// Homebrew or nix bash — would remove both guarantees without changing a
/// line of the checks, and no other test would notice. This is that test.
#[cfg(unix)]
#[test]
fn the_shim_and_the_launch_check_pin_the_same_interpreter() {
    use super::merge_guard::SHIM_INTERPRETER;
    let shim = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(GUARD_REL)
        .join("gh");
    let text = fs::read_to_string(&shim).expect("the shipped merge-guard shim");

    assert_eq!(SHIM_INTERPRETER, "/bin/bash");
    let shebang = text.lines().next().expect("the shim is not empty");
    assert_eq!(
        shebang, "#!/bin/bash -p",
        "the shim's shebang must name the pinned interpreter by absolute path, and keep \
         privileged mode; `#!/usr/bin/env bash -p` would pass `bash -p` to env as one word"
    );
    assert_eq!(
        shebang.trim_start_matches("#!").split(' ').next(),
        Some(SHIM_INTERPRETER),
        "the shim runs under a different bash from the one the launch check verifies"
    );
    let pinned: Vec<&str> = text
        .lines()
        .filter(|l| l.trim_start().starts_with("SHIM_BASH="))
        .collect();
    assert_eq!(
        pinned,
        vec!["SHIM_BASH='/bin/bash'"],
        "the shim must pin SHIM_BASH, exactly once, to {SHIM_INTERPRETER}"
    );
}

/// Crusty round 8: the BASH_ENV file is part of the guard. Without it a
/// profile that reorders PATH puts another gh first, so its absence, or a
/// copy others can write, stops the run like a missing shim.
#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_missing_or_writable_bash_env_file() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let bash_env = home.join(GUARD_REL).join(BASH_ENV_FILE);
    set_mode(&bash_env, 0o664);
    assert_refused_as_writable(&home, "a group-writable bash-env");
    fs::remove_file(&bash_env).unwrap();
    let err = locate_guard_dir(&home)
        .expect_err("a guard without its bash-env is refused")
        .to_string();
    assert!(err.contains("bash-env") && err.contains("#1551"), "{err}");
}

/// The guard's BASH_ENV is exported, and a user BASH_ENV is kept for the
/// guard's file to source first; the guard's own file is never kept as the
/// user's, so a nested run does not source itself.
#[cfg(unix)]
#[test]
fn apply_guard_exports_bash_env_and_keeps_the_users() {
    use super::merge_guard::{USER_BASH_ENV, apply_guard};
    use crate::env_builder::EnvBuilder;
    let dir = Path::new("/opt/ah/amplifier-bundle/tools/gh-merge-guard");
    let ours = dir.join(BASH_ENV_FILE).display().to_string();

    let env = apply_guard(EnvBuilder::new().set("BASH_ENV", "/home/u/.bash_env"), dir).build();
    assert_eq!(env.get("BASH_ENV"), Some(&ours));
    assert_eq!(
        env.get(USER_BASH_ENV).map(String::as_str),
        Some("/home/u/.bash_env")
    );
    assert_eq!(env.get(MERGE_GUARD_ENV).map(String::as_str), Some("1"));

    let nested = apply_guard(EnvBuilder::new().set("BASH_ENV", ours.clone()), dir).build();
    assert_eq!(nested.get("BASH_ENV"), Some(&ours));
    assert_ne!(nested.get(USER_BASH_ENV), Some(&ours));
}
