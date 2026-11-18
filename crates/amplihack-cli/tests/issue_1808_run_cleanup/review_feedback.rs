//! Issue #2785 review regressions exercise deletion through the real CLI.

use super::fixture::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn leftover(fx: &Fx) -> PathBuf {
    let folder = fx.tmp.join("amplihack-workflow-review");
    fs::create_dir_all(&folder).unwrap();
    folder
}

fn bare_clone(fx: &Fx, folder: &Path) -> PathBuf {
    let metadata = folder.join("metadata");
    fx.git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            "--bare",
            fx.repo.to_str().unwrap(),
            metadata.to_str().unwrap(),
        ],
    );
    metadata
}

fn apply(fx: &Fx, folder: &Path) -> serde_json::Value {
    age_tree(folder, 72);
    entry_for(
        &fx.cleanup_json(&["--apply", "--only", "tmp-leftover"]),
        folder,
    )
}

fn assert_preserved(entry: &serde_json::Value, metadata: &Path, reason: &str) {
    assert_eq!(entry["action"], "skipped", "{entry}");
    assert!(
        entry["reason"].as_str().unwrap().contains(reason),
        "{entry}"
    );
    assert!(
        metadata.join("config").is_file(),
        "metadata was removed: {entry}"
    );
}

#[test]
fn review_symlinked_bare_metadata_keeps_local_only_commits() {
    for name in ["refs", "objects", "HEAD"] {
        let fx = Fx::new();
        let folder = leftover(&fx);
        let pushed = fx.git_ok(&fx.repo, &["rev-parse", "HEAD"]);
        fs::write(fx.repo.join("local-only.txt"), "only local\n").unwrap();
        fx.git_ok(&fx.repo, &["add", "local-only.txt"]);
        fx.git_ok(&fx.repo, &["commit", "-qm", "local-only work"]);
        let metadata = bare_clone(&fx, &folder);
        fx.git_ok(
            &metadata,
            &["remote", "set-url", "origin", fx.origin.to_str().unwrap()],
        );
        let sha = fx.git_ok(&metadata, &["rev-parse", "HEAD"]);
        fx.git_ok(
            &metadata,
            &["update-ref", "refs/remotes/origin/main", &pushed],
        );
        assert!(
            !fx.git(&fx.origin, &["cat-file", "-e", &sha])
                .output()
                .unwrap()
                .status
                .success()
        );
        let target = fx.root.join(format!("external-{name}"));
        if name == "HEAD" {
            fs::remove_file(metadata.join(name)).unwrap();
            // Git supports the historical HEAD symlink to a ref, rather
            // than a symlink to another file containing a symbolic ref.
            symlink("refs/heads/main", metadata.join(name)).unwrap();
        } else {
            fs::rename(metadata.join(name), &target).unwrap();
            symlink(&target, metadata.join(name)).unwrap();
        }
        assert_eq!(
            fx.git_ok(
                &metadata,
                &["rev-list", "--count", "HEAD", "--not", "--remotes"]
            ),
            "1"
        );

        let entry = apply(&fx, &folder);
        assert_preserved(
            &entry,
            &metadata,
            "has no upstream and commits on no remote branch",
        );
        assert!(
            fs::symlink_metadata(metadata.join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fx.git_ok(&metadata, &["rev-parse", "HEAD"]), sha);
        assert!(
            fx.git_ok(&metadata, &["show", "HEAD:file.txt"])
                .contains("base")
        );
        assert_eq!(
            fx.git_ok(&metadata, &["show", "HEAD:local-only.txt"]),
            "only local"
        );

        // Positive control: once reachable from a remote-tracking branch,
        // the same linked metadata is fully inspected and can be removed.
        fx.git_ok(
            &metadata,
            &[
                "push",
                "-q",
                fx.origin.to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        fx.git_ok(&metadata, &["update-ref", "refs/remotes/origin/main", &sha]);
        let entry = apply(&fx, &folder);
        assert_eq!(entry["action"], "removed", "{entry}");
        assert!(!folder.exists());
        if name != "HEAD" {
            assert!(target.exists(), "cleanup must not follow the metadata link");
        }
    }
}

#[test]
fn review_incomplete_metadata_preserves_files_and_reports_inspection_failure() {
    for name in ["HEAD", "refs", "objects"] {
        let fx = Fx::new();
        let folder = leftover(&fx);
        let metadata = bare_clone(&fx, &folder);
        let sha = fx.git_ok(&metadata, &["rev-parse", "HEAD"]);
        let backup = fx.root.join("saved-entry");
        fs::rename(metadata.join(name), &backup).unwrap();
        if name != "HEAD" {
            symlink(fx.root.join("missing"), metadata.join(name)).unwrap();
        }

        let entry = apply(&fx, &folder);
        assert_preserved(&entry, &metadata, "cannot inspect repository metadata");
        if name != "HEAD" {
            fs::remove_file(metadata.join(name)).unwrap();
        }
        fs::rename(backup, metadata.join(name)).unwrap();
        assert_eq!(fx.git_ok(&metadata, &["rev-parse", "HEAD"]), sha);
    }
}

fn separate_clone(fx: &Fx, folder: &Path) -> (PathBuf, PathBuf) {
    let metadata = folder.join("metadata");
    let checkout = fx.tmp.join("external-checkout");
    fx.git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            "--separate-git-dir",
            metadata.to_str().unwrap(),
            fx.origin.to_str().unwrap(),
            checkout.to_str().unwrap(),
        ],
    );
    (metadata, checkout)
}

#[test]
fn review_separate_git_dir_checks_dirty_and_unpushed_work_independently() {
    for relative in [false, true] {
        let fx = Fx::new();
        let folder = leftover(&fx);
        let (metadata, checkout) = separate_clone(&fx, &folder);
        let configured = if relative {
            "../../external-checkout"
        } else {
            checkout.to_str().unwrap()
        };
        fx.git_ok(&checkout, &["config", "core.worktree", configured]);
        fs::write(checkout.join("local.txt"), "valuable work\n").unwrap();

        let entry = apply(&fx, &folder);
        assert_preserved(&entry, &metadata, "has uncommitted changes");
        assert_eq!(
            fs::read_to_string(checkout.join("local.txt")).unwrap(),
            "valuable work\n"
        );
        fx.git_ok(&checkout, &["add", "local.txt"]);
        fx.git_ok(&checkout, &["commit", "-qm", "local work"]);
        let sha = fx.git_ok(&checkout, &["rev-parse", "HEAD"]);
        assert!(fx.git_ok(&checkout, &["status", "--porcelain"]).is_empty());

        let entry = apply(&fx, &folder);
        assert_preserved(&entry, &metadata, "has unpushed commits");
        assert_eq!(fx.git_ok(&checkout, &["rev-parse", "HEAD"]), sha);
        assert_eq!(
            fx.git_ok(&checkout, &["show", "HEAD:local.txt"]),
            "valuable work"
        );

        fx.git_ok(&checkout, &["push", "-q", "origin", "HEAD"]);
        let entry = apply(&fx, &folder);
        assert_eq!(entry["action"], "removed", "{entry}");
        assert!(!folder.exists());
    }
}

#[test]
fn review_separate_git_dir_without_a_usable_checkout_is_preserved() {
    for configured in [None, Some("../../missing-checkout"), Some("../../../../")] {
        let fx = Fx::new();
        let folder = leftover(&fx);
        let (metadata, checkout) = separate_clone(&fx, &folder);
        if let Some(configured) = configured {
            fx.git_ok(&checkout, &["config", "core.worktree", configured]);
        }
        let entry = apply(&fx, &folder);
        assert_preserved(&entry, &metadata, "cannot inspect repository metadata");
        assert!(checkout.join("file.txt").is_file());
        fx.git_ok(
            &fx.root,
            &[
                "config",
                "--file",
                metadata.join("config").to_str().unwrap(),
                "core.worktree",
                checkout.to_str().unwrap(),
            ],
        );
        assert!(fx.git_ok(&checkout, &["status", "--porcelain"]).is_empty());
    }
}

#[test]
fn review_filter_diagnostics_redact_quoted_credentials_before_reporting() {
    for scheme in ["https", "http", "ssh", "git", "git+ssh", "ftp", "ftps"] {
        let fx = Fx::new();
        let folder = leftover(&fx);
        let checkout = folder.join("checkout");
        fx.git_ok(
            &fx.root,
            &[
                "clone",
                "-q",
                fx.origin.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        fs::write(checkout.join(".gitattributes"), "file.txt filter=fail\n").unwrap();
        let filter = fx.root.join("failing-filter");
        fs::write(&filter, format!("#!/bin/sh\necho \"fatal: URL ('{scheme}://synthetic-user:synthetic-secret@example.invalid/repo')\" >&2\nexit 1\n")).unwrap();
        fs::set_permissions(&filter, fs::Permissions::from_mode(0o700)).unwrap();
        fx.git_ok(
            &checkout,
            &["config", "filter.fail.clean", filter.to_str().unwrap()],
        );
        fx.git_ok(&checkout, &["config", "filter.fail.required", "true"]);
        // Keep the tracked file's length: Git must run the clean filter to
        // compare contents, rather than declaring it dirty from size alone.
        fs::write(checkout.join("file.txt"), "work\n").unwrap();

        let entry = apply(&fx, &folder);
        assert_preserved(&entry, &checkout.join(".git"), "cannot inspect dirty state");
        let serialized = entry.to_string();
        assert!(!serialized.contains("synthetic-secret"), "{serialized}");
        assert!(!serialized.contains("synthetic-user"), "{serialized}");
        assert!(
            serialized.contains("REDACTED@example.invalid"),
            "{serialized}"
        );
        assert_eq!(
            fs::read_to_string(checkout.join("file.txt")).unwrap(),
            "work\n"
        );
    }
}

#[test]
fn review_text_output_escapes_paths_and_json_retains_actual_values() {
    let fx = Fx::new();
    let name = "amplihack-workflow-escape\u{1b}[31m\n\t\u{7f}\u{9b}";
    let folder = fx.tmp.join(name);
    fs::create_dir_all(folder.join(".git")).unwrap();
    age_tree(&folder, 72);
    let report = fx.cleanup_json(&["--only", "tmp-leftover"]);
    assert_eq!(
        entry_for(&report, &folder)["path"],
        folder.to_str().unwrap()
    );
    for args in [
        vec!["--only", "tmp-leftover"],
        vec!["--apply", "--only", "tmp-leftover"],
    ] {
        let out = fx.cleanup(&args);
        assert!(out.status.success(), "{}", describe(&out));
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("skipped"), "{text}");
        assert!(
            !text.chars().any(|c| c.is_control() && c != '\n'),
            "{text:?}"
        );
        assert!(text.contains("\\n\\t"), "{text:?}");
    }
    fs::remove_dir(folder.join(".git")).unwrap();
    age_tree(&folder, 72);
    let out = fx.cleanup(&["--apply", "--only", "tmp-leftover"]);
    assert!(out.status.success(), "{}", describe(&out));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("removed"), "{text}");
    assert!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "{text:?}"
    );
    assert!(!folder.exists());

    fs::create_dir_all(&folder).unwrap();
    age_tree(&folder, 72);
    fs::set_permissions(&fx.tmp, fs::Permissions::from_mode(0o500)).unwrap();
    let out = fx.cleanup(&["--apply", "--only", "tmp-leftover"]);
    fs::set_permissions(&fx.tmp, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("failed"), "{text}");
    assert!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "{text:?}"
    );
    assert!(folder.exists());
}
