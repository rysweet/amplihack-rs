//! Group (d2), SR2: when `probe-terminal-state` finds no trusted copy of
//! `workflow_github_repo.sh`, its FAILED_INVALID_INPUT reason says what to run
//! (`amplihack install`) and lists every searched path once, in lookup order,
//! each with an exact marker: `(missing)`, `(unreadable)` or
//! `(skipped: inside target checkout)`. The reason never carries the origin.

use super::fixture::{Fixture, Outcome, helper_path, write_executable};
use super::recipe_lookup::run_probe_bare;
use std::fs;
use std::path::{Path, PathBuf};

const HELPER: &str = "workflow_github_repo.sh";
const MISSING: &str = "(missing)";
const UNREADABLE: &str = "(unreadable)";
const SKIPPED: &str = "(skipped: inside target checkout)";

/// `(path, marker)` pairs from the `Searched:` list of the failure reason.
fn searched(fx: &Fixture, out: &Outcome) -> Vec<(String, String)> {
    let json = out
        .json()
        .unwrap_or_else(|| panic!("no terminal JSON\n{}", out.describe()));
    assert_eq!(
        json["terminal_state"],
        "FAILED_INVALID_INPUT",
        "{}",
        out.describe()
    );
    let reason = json["terminal_reason"]
        .as_str()
        .expect("reason")
        .to_string();
    assert!(
        reason.contains("run 'amplihack install'"),
        "reason must say what to run: {reason}"
    );
    assert!(
        out.stderr.contains(&format!(
            "ERROR: workflow-terminal-state: FAILED_INVALID_INPUT: {reason}"
        )),
        "{}",
        out.describe()
    );
    assert!(
        fx.repo_gh_calls().is_empty(),
        "gh calls {:?}",
        fx.repo_gh_calls()
    );
    let (_, list) = reason
        .split_once("Searched: ")
        .unwrap_or_else(|| panic!("reason must list searched paths: {reason}"));
    list.split("; ")
        .map(|entry| {
            let (path, marker) = entry
                .rsplit_once(" (")
                .unwrap_or_else(|| panic!("entry without marker: {entry:?}"));
            (path.to_string(), format!("({marker}"))
        })
        .collect()
}

fn both_spellings(dir: &Path) -> [String; 2] {
    let phys = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    [
        dir.join(HELPER).display().to_string(),
        phys.join(HELPER).display().to_string(),
    ]
}

fn find<'a>(entries: &'a [(String, String)], dir: &Path) -> Vec<&'a (String, String)> {
    let names = both_spellings(dir);
    entries.iter().filter(|(p, _)| names.contains(p)).collect()
}

fn tools(root: &Path) -> PathBuf {
    root.join("amplifier-bundle/tools")
}

#[test]
fn reason_lists_installed_bundle_paths_as_missing_in_lookup_order() {
    let fx = Fixture::with_origin("https://x:FAKE1642SECRET@github.com/o/r.git");
    let out = run_probe_bare(&fx, &[]);
    let entries = searched(&fx, &out);
    let home = fx.path("empty-home");
    let copilot = find(&entries, &home.join(".copilot/amplifier-bundle/tools"));
    let amplihack = find(&entries, &home.join(".amplihack/amplifier-bundle/tools"));
    assert_eq!(copilot.len(), 1, "{entries:?}");
    assert_eq!(amplihack.len(), 1, "{entries:?}");
    assert_eq!(copilot[0].1, MISSING);
    assert_eq!(amplihack[0].1, MISSING);
    let pos = |e: &(String, String)| entries.iter().position(|x| x == e).unwrap();
    let repo_pos = pos(find(&entries, &tools(&fx.repo))[0]);
    assert!(repo_pos < pos(copilot[0]) && pos(copilot[0]) < pos(amplihack[0]));
    assert!(
        !out.stderr.contains("FAKE1642SECRET") && !out.stdout.contains("FAKE1642SECRET"),
        "the origin must never be printed\n{}",
        out.describe()
    );
}

#[test]
fn a_path_reached_through_repo_path_and_cwd_is_listed_once() {
    // REPO_PATH equals the cwd (the fixture runs in fx.repo).
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let out = run_probe_bare(&fx, &[]);
    let entries = searched(&fx, &out);
    assert_eq!(find(&entries, &tools(&fx.repo)).len(), 1, "{entries:?}");
}

#[test]
fn a_helper_inside_the_checkout_is_marked_skipped() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let planted = tools(&fx.repo).join(HELPER);
    write_executable(
        &planted,
        "parse_github_repo_identity() { printf 'o/r\\n'; }\n",
    );
    let out = run_probe_bare(&fx, &[]);
    let entries = searched(&fx, &out);
    let hits = find(&entries, &tools(&fx.repo));
    assert_eq!(hits.len(), 1, "{entries:?}");
    assert_eq!(hits[0].1, SKIPPED, "{entries:?}");
}

#[test]
fn an_unreadable_installed_helper_is_marked_unreadable() {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .expect("id -u");
    if String::from_utf8_lossy(&uid.stdout).trim() == "0" {
        eprintln!("skipped: root can read a mode-000 file");
        return;
    }
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let home = fx.path("fake-amplihack-home");
    let dir = tools(&home);
    fs::create_dir_all(&dir).expect("mkdir");
    let file = dir.join(HELPER);
    fs::copy(helper_path(), &file).expect("copy helper");
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    }
    let out = run_probe_bare(&fx, &[("AMPLIHACK_HOME", home.as_os_str())]);
    let entries = searched(&fx, &out);
    let hits = find(&entries, &dir);
    assert_eq!(hits.len(), 1, "{entries:?}");
    assert_eq!(hits[0].1, UNREADABLE, "{entries:?}");
    // AMPLIHACK_HOME is searched first.
    assert_eq!(entries.first(), Some(hits[0]), "{entries:?}");
}

#[test]
fn every_entry_uses_one_of_the_three_exact_markers() {
    let fx = Fixture::with_origin("https://github.com/o/r.git");
    let out = run_probe_bare(&fx, &[]);
    let entries = searched(&fx, &out);
    assert!(!entries.is_empty());
    for (path, marker) in &entries {
        assert!(
            path.ends_with(&format!("/amplifier-bundle/tools/{HELPER}")),
            "{path}"
        );
        assert!(
            [MISSING, UNREADABLE, SKIPPED].contains(&marker.as_str()),
            "unexpected marker {marker:?} for {path}"
        );
    }
}
