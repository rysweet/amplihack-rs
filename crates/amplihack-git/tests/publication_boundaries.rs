//! Real-Git behavioural checks independent of the large CLI test target.
use amplihack_git::{checkpoint_transaction, publication_validation, runtime_preparation};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn child(test: &str, body: impl FnOnce()) {
    if std::env::var_os("AMPLIHACK_1554_GIT_CHILD").is_some() {
        body();
        return;
    }
    let temp = tempfile::tempdir_in("/var/tmp").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    amplihack_git::scrub(&mut command);
    command
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env("AMPLIHACK_1554_GIT_CHILD", "1")
        .env("TMPDIR", temp.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "AMPLIHACK_HOME",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "AMPLIHACK_CLAUDE_PROFILE_ROOT",
    ] {
        let path = temp.path().join(key);
        fs::create_dir(&path).unwrap();
        command.env(key, path);
    }
    let scope = r#"{"version":1,"task":"1554","permitted_source_paths":["source.txt","new.txt"]}"#;
    command.env("RECIPE_VAR_final_requirements", scope).env(
        "RECIPE_VAR_design_spec",
        r#"{"version":1,"task":"1554","files_to_change":["source.txt","new.txt"]}"#,
    );
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn git(root: &Path, args: &[&str]) -> String {
    let output = amplihack_git::command_in(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let remote = temp.path().join("remote.git");
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "--quiet", "--template=", "-b", "main"]);
        git(&root, &["config", "user.name", "Fixture"]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        git(&root, &["config", "commit.gpgsign", "false"]);
        fs::write(root.join("source.txt"), "initial\n").unwrap();
        fs::write(root.join("unrelated.txt"), "initial\n").unwrap();
        git(&root, &["add", "--", "source.txt", "unrelated.txt"]);
        git(&root, &["commit", "-qm", "initial"]);
        git(
            &root,
            &[
                "init",
                "--bare",
                "--quiet",
                "--template=",
                remote.to_str().unwrap(),
            ],
        );
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&root, &["push", "--quiet", "-u", "origin", "HEAD"]);
        Self {
            _temp: temp,
            root,
            remote,
        }
    }
    fn commit(&self, path: &str, content: &str) -> String {
        fs::write(self.root.join(path), content).unwrap();
        git(&self.root, &["add", "--", path]);
        git(&self.root, &["commit", "-qm", "change"]);
        git(&self.root, &["rev-parse", "HEAD"])
    }
}
#[test]
fn intermediate_changes_and_forged_tracking_refs_cannot_authorise_publication() {
    child(
        "intermediate_changes_and_forged_tracking_refs_cannot_authorise_publication",
        || {
            let repo = Repo::new();
            let before = git(&repo.remote, &["rev-parse", "refs/heads/main"]);
            let bad = repo.commit("unrelated.txt", "unauthorised\n");
            git(&repo.root, &["revert", "--no-edit", &bad]);
            git(
                &repo.root,
                &["update-ref", "refs/remotes/origin/main", "HEAD"],
            );
            let error = publication_validation::publish(&repo.root)
                .unwrap_err()
                .to_string();
            assert!(error.contains("unrelated.txt"), "{error}");
            assert_eq!(git(&repo.remote, &["rev-parse", "refs/heads/main"]), before);
        },
    );
}
#[test]
fn advertised_siblings_and_remote_merges_are_valid_but_resolution_changes_are_not() {
    child(
        "advertised_siblings_and_remote_merges_are_valid_but_resolution_changes_are_not",
        || {
            for bad_resolution in [false, true] {
                let repo = Repo::new();
                git(&repo.root, &["checkout", "--quiet", "-b", "imported"]);
                let imported = repo.commit("unrelated.txt", "advertised\n");
                git(&repo.root, &["push", "--quiet", "origin", "HEAD"]);
                git(&repo.root, &["checkout", "--quiet", "main"]);
                repo.commit("new.txt", "authorised\n");
                git(&repo.root, &["merge", "--no-ff", "--no-commit", &imported]);
                if bad_resolution {
                    fs::write(repo.root.join("unrelated.txt"), "bad resolution\n").unwrap();
                    git(&repo.root, &["add", "--", "unrelated.txt"]);
                }
                git(&repo.root, &["commit", "-qm", "merge remote history"]);
                let result = publication_validation::publish(&repo.root);
                if bad_resolution {
                    assert!(result.unwrap_err().to_string().contains("unrelated.txt"));
                } else {
                    result.unwrap();
                    assert_eq!(
                        git(&repo.remote, &["rev-parse", "main"]),
                        git(&repo.root, &["rev-parse", "HEAD"])
                    );
                }
            }
        },
    );
}
#[test]
fn automatic_staging_keeps_protected_blob_and_excludes_runtime() {
    child(
        "automatic_staging_keeps_protected_blob_and_excludes_runtime",
        || {
            let repo = Repo::new();
            runtime_preparation::prepare(&repo.root).unwrap();
            fs::write(repo.root.join("unrelated.txt"), "protected\n").unwrap();
            git(&repo.root, &["add", "--", "unrelated.txt"]);
            fs::write(repo.root.join("unrelated.txt"), "unstaged\n").unwrap();
            fs::write(repo.root.join("new.txt"), "authorised\n").unwrap();
            fs::create_dir_all(repo.root.join(".amplihack")).unwrap();
            fs::write(repo.root.join(".amplihack/indexing.pid"), "1\n").unwrap();
            checkpoint_transaction::checkpoint(&repo.root, "stage", true).unwrap();
            assert_eq!(git(&repo.root, &["show", ":unrelated.txt"]), "protected");
            assert_eq!(git(&repo.root, &["show", ":new.txt"]), "authorised");
            assert!(!git(&repo.root, &["ls-files"]).contains("indexing.pid"));
        },
    );
}
