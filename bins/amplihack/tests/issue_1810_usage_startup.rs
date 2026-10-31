//! Exercise the actual binary: reporting must not repair/install assets or change inputs.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, current: &Path, result: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                result.insert(relative, None);
                visit(root, &path, result);
            } else {
                result.insert(relative, Some(fs::read(path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn report_is_read_only_with_empty_or_stale_install_and_no_external_tools() {
    for stale in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let empty_bin = root.path().join("empty-bin");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&empty_bin).unwrap();
        if stale {
            fs::create_dir(home.join(".amplihack")).unwrap();
            fs::write(home.join(".amplihack/.installed-version"), "0.0.0\n").unwrap();
        }
        let log = json!({"type":"assistant","sessionId":"s","timestamp":"2026-01-01T00:00:00Z",
            "message":{"id":"message","model":"test-model","content":"private-fixture-content",
                "usage":{"input_tokens":12,"cache_read_input_tokens":4,"cache_creation_input_tokens":3,
                    "cache_creation":{"ephemeral_5m_input_tokens":2,"ephemeral_1h_input_tokens":1},"output_tokens":5}}});
        fs::write(root.path().join("native.jsonl"), log.to_string()).unwrap();
        let manifest = root.path().join("usage.json");
        fs::write(&manifest,json!({"version":1,"workstream":"ws","repository":"owner/repo","branch":"branch","prs":[42],
            "sessions":[{"id":"s","provider":"claude","run_id":"run","complete":true,"files":["native.jsonl"]}]}).to_string()).unwrap();
        let before = inventory(root.path());
        for format in ["json", "summary"] {
            let mut command = crate::isolated_amplihack::isolated(env!("CARGO_BIN_EXE_amplihack"));
            let output = command
                .current_dir(root.path())
                .env("HOME", &home)
                .env("PATH", &empty_bin)
                .env("XDG_CACHE_HOME", home.join(".cache"))
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env_remove("AMPLIHACK_SKIP_AUTO_INSTALL")
                .env_remove("AMPLIHACK_NO_UPDATE_CHECK")
                .args(["usage", "report", "--manifest"])
                .arg(&manifest)
                .args(["--format", format])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "report failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(!text.contains("private-fixture-content"));
            if format == "json" {
                let data: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(data["complete"], true);
                assert_eq!(data["total"]["input"], 12);
                assert_eq!(data["total"]["cache_write"], 3);
                assert_eq!(data["total"]["cache_read"], 4);
                assert_eq!(data["total"]["output"], 5);
            } else {
                assert!(text.contains("Workstream usage: ws"));
            }
            assert!(
                before == inventory(root.path()),
                "report changed isolated input/install state"
            );
        }
    }
}
