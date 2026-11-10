//! Issue #1954 (R6, A2): which binary re-stages the bundle during the first
//! `amplihack update` to this release.
//!
//! An installed 1.2.107 (or any release from 0.9.71 on) updates in two
//! steps. The OLD binary downloads the new one and renames it over
//! `~/.local/bin/amplihack`, then runs it as a child with
//! `install --force-refresh` and `AMPLIHACK_POST_UPDATE_INSTALL=1`
//! (`v1.2.107:crates/amplihack-cli/src/update/check.rs:361-397`, pinned by
//! `update/tests/build_install_command.rs::exact_handoff_contract_matches_v1_2_107`).
//!
//! The test plays the OLD binary with a stub script that does exactly those
//! two steps and nothing else, with the binary built from this tree as the
//! "downloaded" one. It cannot download, so `AMPLIHACK_HOME` points at a tag
//! tree, as in the host test. A live run is parked in the legacy bundle
//! (a real directory, the layout 1.2.107 leaves behind).
//!
//! The evidence is the `.installed-by` record the install writes into the
//! release it commits: its `pid` must be the pid of the NEW binary's
//! process (the stub's child), not the stub's own, and `post_update=1`.
//! If this holds, the first update from 1.2.107 needs no window without
//! recipe runs: the old binary only renames a file, which leaves every
//! inode a running process holds intact.
//!
//! The stub also lists `~/.amplihack` just before it starts the new
//! binary. That listing must equal the one the test takes before it starts
//! the stub: the old binary's part of the hand-off writes nothing there.

#![cfg(target_os = "linux")]

use crate::issue_1954_install_leaves_live_runs_alone::{
    Host, RUN_SCRIPT, plant_legacy_layout, proc_link, published_release, text,
};
use crate::support::amplihack_bin::amplihack_bin;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The OLD binary: what an installed 1.2.107 does in `amplihack update`
/// after the download. `$NEW_BIN` is the "downloaded" binary. It writes only
/// to `$work` and to `~/.local/bin/amplihack` (by rename), never under
/// `~/.amplihack`. The install runs as a child, not through `exec`, so the
/// stub's pid and the new binary's pid differ. `$2` is where it writes its
/// listing of `~/.amplihack`, taken just before it starts the new binary.
const OLD_BINARY_STUB: &str = r#"set -euo pipefail
work="$1"
listing="$2"
echo $$ > "$work/old.pid"
bin="$HOME/.local/bin"
cp "$NEW_BIN" "$bin/amplihack.tmp"
chmod 755 "$bin/amplihack.tmp"
mv -f "$bin/amplihack.tmp" "$bin/amplihack"
echo "old: renamed the new binary over $bin/amplihack" >> "$work/old.trace"
{ find "$HOME/.amplihack" -print 2>/dev/null || true; } | LC_ALL=C sort > "$listing"
AMPLIHACK_NO_UPDATE_CHECK=1 AMPLIHACK_NONINTERACTIVE=1 AMPLIHACK_POST_UPDATE_INSTALL=1 \
  "$bin/amplihack" install --force-refresh &
new=$!
echo "$new" > "$work/new.pid"
echo "old: started $bin/amplihack install --force-refresh as pid $new" >> "$work/old.trace"
wait "$new"
"#;

/// A version below this build's, for the legacy stamp: the previous patch,
/// or 0.9.71 (the first release that sets `AMPLIHACK_POST_UPDATE_INSTALL`).
fn older_version() -> String {
    let mut parts = amplihack_cli::VERSION
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse::<u64>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    if patch > 0 {
        format!("{major}.{minor}.{}", patch - 1)
    } else {
        "0.9.71".to_string()
    }
}

/// `.installed-by` parsed by hand, so the test checks the file format and
/// not only the crate's own reader: `key=value` lines, split on the first
/// `=`, first occurrence wins.
fn parse_record(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            map.entry(key.to_string())
                .or_insert_with(|| value.to_string());
        }
    }
    map
}

/// Every path under `root`, `root` included, as `find -print` names them.
/// Symlinks are listed, not followed. A missing `root` gives an empty set.
fn listing(root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir()
            && let Ok(entries) = fs::read_dir(&path)
        {
            stack.extend(entries.flatten().map(|e| e.path()));
        }
        found.insert(path.to_string_lossy().into_owned());
    }
    found
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(target_os = "linux")]
#[test]
fn the_new_binary_restages_on_the_first_update_and_a_live_run_survives() {
    let host = Host::new();
    plant_legacy_layout(&host);
    let stamp = host.home.join(".amplihack/.installed-version");
    fs::write(&stamp, format!("{}\n", older_version())).unwrap();
    let legacy_bundle = host.bundle();
    assert!(
        fs::symlink_metadata(&legacy_bundle)
            .unwrap()
            .file_type()
            .is_dir(),
        "the legacy layout has a real bundle directory"
    );

    let run = host.park_script("a", RUN_SCRIPT, &[]);
    let a_pid = run.pid();

    // The old binary's update step, in a clean environment inside the temp
    // dir (S-D1..S-D3).
    host.assert_contained();
    let stub = host.work.join("old-binary.sh");
    fs::write(&stub, OLD_BINARY_STUB).unwrap();
    let new_bin = amplihack_bin();
    let dot_amplihack = host.home.join(".amplihack");
    let listing_before = listing(&dot_amplihack);
    let stub_listing = host.work.join("old-binary.listing");
    let output = Command::new("/bin/bash")
        .arg(&stub)
        .arg(&host.work)
        .arg(&stub_listing)
        .env_clear()
        .env("HOME", &host.home)
        .env("TMPDIR", host.temp.path())
        .envs(host.install_env())
        .env("NEW_BIN", &new_bin)
        .env("AMPLIHACK_TEST_HOME_GUARD", "1")
        .env("AMPLIHACK_ROTATE_AUTOSTART", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .current_dir(&host.work)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    let cwd_during = proc_link(&a_pid, "cwd");
    let run_status = run.finish();

    let mut failures: Vec<String> = Vec::new();
    if !output.status.success() {
        failures.push(format!("the update hand-off failed: {}", text(&output)));
    }
    let old_pid = host.work_file("old.pid").trim().to_string();
    let new_pid = host.work_file("new.pid").trim().to_string();
    assert!(
        !old_pid.is_empty() && !new_pid.is_empty() && old_pid != new_pid,
        "the stub must record two different pids (old {old_pid:?}, new {new_pid:?}); \
         trace:\n{}",
        host.work_file("old.trace")
    );

    // The old binary wrote nothing under ~/.amplihack before it started the
    // new one.
    match fs::read_to_string(&stub_listing) {
        Err(e) => failures.push(format!("the stub wrote no listing: {e}")),
        Ok(stub_text) => {
            let at_handoff: BTreeSet<String> = stub_text.lines().map(str::to_string).collect();
            let added: Vec<&String> = at_handoff.difference(&listing_before).collect();
            let removed: Vec<&String> = listing_before.difference(&at_handoff).collect();
            if !added.is_empty() || !removed.is_empty() {
                failures.push(format!(
                    "the old binary changed ~/.amplihack before the hand-off: \
                     added {added:?}, removed {removed:?}"
                ));
            }
        }
    }

    // The bundle link names a complete release of this version.
    let release = match published_release(&host, &host.bundle(), "amplifier-bundle") {
        Ok(release) => Some(release),
        Err(e) => {
            failures.push(format!("the bundle was not switched to a release: {e}"));
            None
        }
    };

    // The provenance record names the new binary's process.
    if let Some(release) = &release {
        let record_path = release.join(".installed-by");
        match fs::read_to_string(&record_path) {
            Err(e) => failures.push(format!(
                "no provenance record at {}: {e}",
                record_path.display()
            )),
            Ok(record_text) => {
                let record = parse_record(&record_text);
                let field = |k: &str| record.get(k).map(String::as_str).unwrap_or("");
                if field("format") != "1" {
                    failures.push(format!("format is {:?}, not 1", field("format")));
                }
                if field("version") != amplihack_cli::VERSION {
                    failures.push(format!(
                        "version is {:?}, not {}",
                        field("version"),
                        amplihack_cli::VERSION
                    ));
                }
                if field("post_update") != "1" {
                    failures.push(format!(
                        "post_update is {:?}: the install did not run as the hand-off child",
                        field("post_update")
                    ));
                }
                if field("pid") == old_pid {
                    failures.push(format!(
                        "pid {old_pid} is the OLD binary's: the old binary re-staged \
                         the bundle, so the first install needs a window without runs"
                    ));
                }
                if field("pid") != new_pid {
                    failures.push(format!(
                        "pid is {:?}, not the new binary's process {new_pid}",
                        field("pid")
                    ));
                }
                let exe = PathBuf::from(field("exe"));
                let allowed = [
                    canonical(&host.home.join(".local/bin/amplihack")),
                    host.home.join(".local/bin/amplihack"),
                    canonical(&new_bin),
                    canonical(&release.join("bin/amplihack")),
                ];
                if field("exe").is_empty() || !allowed.contains(&exe) {
                    failures.push(format!(
                        "exe is {:?}; expected the renamed-in binary \
                         ($HOME/.local/bin/amplihack), the built binary or the \
                         release's bin/amplihack",
                        field("exe")
                    ));
                }
                if failures.is_empty() {
                    eprintln!(
                        "issue #1954: the first update was re-staged by the new binary \
                         (pid {new_pid}, old stub pid {old_pid}):\n{record_text}"
                    );
                }
            }
        }
    }

    // Only one release was made, and it is the one the record names.
    let made: Vec<String> = fs::read_dir(host.releases())
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    if made.len() != 1 {
        failures.push(format!(
            "expected exactly one release from one install, found {made:?}"
        ));
    }

    // The stamp was written by the new binary's install.
    let stamp_now = fs::read_to_string(&stamp).unwrap_or_default();
    if stamp_now.lines().next() != Some(amplihack_cli::VERSION) {
        failures.push(format!(
            "the stamp is {:?}, not {}",
            stamp_now.lines().next(),
            amplihack_cli::VERSION
        ));
    }

    // The live run in the legacy tree survived.
    if cwd_during.ends_with(" (deleted)") {
        failures.push(format!(
            "the legacy tree under the live run was deleted: {cwd_during}"
        ));
    }
    if !(run_status.success() && host.work_file("a.done").starts_with("DONE ")) {
        failures.push(format!(
            "the live run did not finish normally ({run_status:?}):\n{}",
            host.work_file("a.err")
        ));
    }

    assert!(
        failures.is_empty(),
        "issue #1954 first update: {} failure(s):\n\n{}\n\nold-binary trace:\n{}\n\n{}",
        failures.len(),
        failures.join("\n\n"),
        host.work_file("old.trace"),
        text(&output)
    );
}
