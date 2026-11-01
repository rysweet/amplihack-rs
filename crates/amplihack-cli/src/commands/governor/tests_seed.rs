//! Issue #1811 (TDD): a temporary profile root for the governor's unit
//! tests, shared by `tests_status` and `tests_admit`.
//!
//! Wire with `#[cfg(all(test, unix))] mod tests_seed;` in `governor/mod.rs`.
//!
//! Each profile is a directory with an OAuth login file (so it is a
//! subscription profile) and an entry in `.auto-switch.d/usage.json` written
//! the way the rotator writes it. Nothing here reads the real HOME or the
//! process environment; the root is always passed explicitly.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) const NOW: i64 = 1_800_000_000;
const HOUR: i64 = 3600;

/// `(name, session %, weekly %)`; a negative session means weekly-only.
pub(super) type Seed<'a> = (&'a str, f64, f64);

fn private(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

pub(super) struct Root {
    pub tmp: tempfile::TempDir,
    pub root: PathBuf,
}

impl Root {
    /// A root with `profiles`, the first of them active, and readings for
    /// each. An empty slice gives a root with one profile and no readings.
    pub fn new(profiles: &[Seed]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("profiles");
        private_dir(&root);
        let names: Vec<&str> = if profiles.is_empty() {
            vec!["alpha"]
        } else {
            profiles.iter().map(|p| p.0).collect()
        };
        for name in &names {
            private_dir(&root.join(name));
            private(
                &root.join(name).join(".credentials.json"),
                r#"{"claudeAiOauth":{"accessToken":"PLACEHOLDER"}}"#,
            );
        }
        private(&root.join(".default"), &format!("{}\n", names[0]));
        let r = Self { tmp, root };
        if !profiles.is_empty() {
            r.readings(profiles);
        }
        r
    }

    /// Replaces `usage.json` with these readings, observed at `NOW`.
    pub fn readings(&self, profiles: &[Seed]) {
        let mut accounts = serde_json::Map::new();
        for (name, session, weekly) in profiles {
            let mut reading = serde_json::json!({
                "seven_day": {"used_percentage": weekly, "resets_at": NOW + 48 * HOUR},
                "observed_at": NOW,
                "source": "statusline",
            });
            if *session >= 0.0 {
                reading["five_hour"] =
                    serde_json::json!({"used_percentage": session, "resets_at": NOW + 2 * HOUR});
            }
            accounts.insert(name.to_string(), reading);
        }
        let dir = self.root.join(".auto-switch.d");
        private_dir(&dir);
        let body = serde_json::json!({"version": 1, "accounts": accounts});
        private(&dir.join("usage.json"), &body.to_string());
    }

    pub fn gov(&self) -> PathBuf {
        self.root.join(super::store::GOVERNOR_DIR)
    }

    /// Every path under the temporary directory with its bytes, for
    /// "nothing changed" checks. Directories are listed with no bytes.
    pub fn snapshot(&self) -> Vec<(PathBuf, Option<Vec<u8>>)> {
        fn walk(p: &Path, out: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
            let meta = fs::symlink_metadata(p).unwrap();
            if meta.is_dir() {
                out.push((p.to_path_buf(), None));
                let mut entries: Vec<_> = fs::read_dir(p)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                entries.sort();
                for e in entries {
                    walk(&e, out);
                }
            } else {
                out.push((p.to_path_buf(), fs::read(p).ok()));
            }
        }
        let mut out = Vec::new();
        walk(self.tmp.path(), &mut out);
        out
    }
}

/// The PID of a process that has already exited.
pub(super) fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}
