//! Amplihack's work identity and launch admission; replay belongs to the runner.
use crate::commands::cleanup::recovery_path::display_path;
use anyhow::Result;
use recipe_runner_rs::recovery::{admission::Admission, model::fingerprint, resources::git};
use std::{collections::BTreeMap, path::Path};

fn repository_name(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?;
    let parts: Vec<_> = name.split('/').collect();
    (parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }))
    .then(|| name.to_ascii_lowercase())
}
fn number(value: &str) -> Option<u64> {
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .then(|| value.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n > 0)
}
/// Structural URL validation: no prose extraction, alternate host or conflicting issue.
fn issue(context: &BTreeMap<String, String>, repo: &Path) -> Option<u64> {
    let explicit = context.get("issue_number").filter(|v| !v.is_empty());
    let mut issue = explicit.and_then(|v| number(v));
    if explicit.is_some() && issue.is_none() {
        return None;
    }
    if let Some(url) = context.get("issue_url").filter(|v| !v.is_empty()) {
        let rest = url.strip_prefix("https://github.com/")?;
        let (repository, raw) = rest.split_once("/issues/")?;
        let remote = String::from_utf8(git(repo, &["remote", "get-url", "origin"]).ok()?).ok()?;
        if repository.to_ascii_lowercase() != repository_name(&remote)? {
            return None;
        }
        let from_url = number(raw)?;
        if issue.is_some_and(|n| n != from_url) {
            return None;
        }
        issue = Some(from_url);
    }
    issue
}
fn work_repository(
    working_dir: &Path,
    context: &BTreeMap<String, String>,
) -> Option<std::path::PathBuf> {
    let selected = context
        .get("repo_path")
        .filter(|v| !v.is_empty())
        .map_or(Path::new("."), |v| Path::new(v));
    let path = if selected.is_absolute() {
        selected.to_path_buf()
    } else {
        working_dir.join(selected)
    };
    path.canonicalize().ok()
}
/// Worktrees and symlink aliases share identity; independent clones do not.
pub(super) fn identity(
    recipe: &Path,
    recipe_name: &str,
    repo: &Path,
    context: &BTreeMap<String, String>,
) -> Option<String> {
    let repo = work_repository(repo, context)?;
    let issue = issue(context, &repo)?;
    let common = String::from_utf8(
        git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .ok()?,
    )
    .ok()?;
    let common = Path::new(common.trim()).canonicalize().ok()?;
    let recipe = recipe.canonicalize().ok()?;
    fingerprint(&(recipe_name, recipe, common, issue)).ok()
}
/// Normalize explicit work metadata before evaluating inputs, so a URL and its
/// equivalent numeric issue select compatible context at relaunch.
///
/// Both keys are written: the runner treats every context key a shell step
/// can see as one of its inputs (runner #208), so `issue_url` present on one
/// launch and absent on the other would stop replay even though the issue is
/// the same. A GitHub `origin` gives the canonical URL; otherwise only the
/// number is normalised.
pub(super) fn normalize(context: &mut BTreeMap<String, String>, repo: &Path) {
    let Some(repo) = work_repository(repo, context) else {
        return;
    };
    let Some(issue) = issue(context, &repo) else {
        return;
    };
    context.insert("issue_number".into(), issue.to_string());
    // A failed `get-url` is treated like a missing or non-GitHub origin, as
    // `issue()` does. `work_repository` has already run git in this
    // repository, so in practice the failure is a missing `origin`.
    let repository = git(&repo, &["remote", "get-url", "origin"])
        .ok()
        .and_then(|remote| String::from_utf8(remote).ok())
        .and_then(|remote| repository_name(&remote));
    if let Some(repository) = repository {
        context.insert(
            "issue_url".into(),
            format!("https://github.com/{repository}/issues/{issue}"),
        );
    }
}
pub(super) fn admit(
    workspace: &crate::commands::cleanup::run_dir::RunWorkspace,
    recipe: &Path,
    name: &str,
    repo: &Path,
    context: &BTreeMap<String, String>,
    fresh: bool,
    settings: String,
) -> Result<Admission> {
    let crate::commands::cleanup::run_dir::RunWorkspace::Scoped { scope, .. } = workspace else {
        anyhow::bail!("durable recipe execution requires a private work root");
    };
    let identity = identity(recipe, name, repo, context);
    // Issue #2346: the top-level `Error: {error}` prints only the outermost
    // message, so the runner's reason is written into it, after the work root
    // and the run folder that it concerns.
    Admission::begin(
        &scope.work_root.path,
        &scope.run_dir,
        identity.as_deref(),
        fresh,
        settings,
    )
    .map_err(|error| {
        let diagnostics = crate::commands::cleanup::recovery_path::recovery_diagnostics(
            &scope.work_root.path, Some(&scope.run_dir), identity.as_deref(), &error,
        );
        let message = format!(
            "cannot durably admit recipe launch (work root {}, run folder {}): {diagnostics}; runner: {error:#}",
            display_path(&scope.work_root.path),
            display_path(&scope.run_dir)
        );
        error.context(message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cleanup::{
        run_dir::{RunScope, RunWorkspace},
        work_root::WorkRoot,
    };
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
    use std::path::PathBuf;

    struct Fixture {
        _tmp: tempfile::TempDir,
        workspace: RunWorkspace,
        root: PathBuf,
        repo: PathBuf,
        recipe: PathBuf,
        context: BTreeMap<String, String>,
        _env_lock: std::sync::MutexGuard<'static, ()>,
    }

    fn private_dir(path: &Path) {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .unwrap();
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().mode() & 0o7777
    }

    impl Fixture {
        fn new() -> Self {
            // Other lib tests temporarily replace PATH. Freeze those changes
            // while the real identity calculation invokes Git below.
            let host_path = crate::test_support::host_path();
            let env_lock = crate::test_support::env_lock()
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let tmp = crate::test_support::tempdir_outside_framework_roots();
            let base = tmp.path().canonicalize().unwrap();
            let repo = base.join("repo");
            let home = base.join("home");
            private_dir(&repo);
            private_dir(&home);
            let output = amplihack_git::command_in(&repo)
                .env("PATH", host_path)
                .env("HOME", &home)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(["-c", "init.templateDir=", "init", "--quiet"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let recipe = base.join("recipe.yaml");
            fs::write(&recipe, b"identity fixture, never executed").unwrap();
            let root = base.join("work");
            let (scope, guard) = RunScope::begin(
                WorkRoot {
                    path: root.clone(),
                    explicit: true,
                },
                "issue-2346",
                &repo,
            )
            .unwrap();
            Self {
                _tmp: tmp,
                workspace: RunWorkspace::Scoped {
                    scope: Box::new(scope),
                    guard,
                },
                root,
                repo,
                recipe,
                context: BTreeMap::from([("issue_number".into(), "2346".into())]),
                _env_lock: env_lock,
            }
        }

        fn run(&self) -> &Path {
            self.workspace.run_dir().unwrap()
        }

        fn index(&self) -> PathBuf {
            let identity = identity(&self.recipe, "issue-2346", &self.repo, &self.context).unwrap();
            self.root
                .join("recovery-index")
                .join(recipe_runner_rs::recovery::model::digest(
                    identity.as_bytes(),
                ))
        }

        fn admit(&self) -> Result<Admission> {
            admit(
                &self.workspace,
                &self.recipe,
                "issue-2346",
                &self.repo,
                &self.context,
                false,
                "fixture-settings".into(),
            )
        }

        fn error(&self) -> anyhow::Error {
            match self.admit() {
                Ok(_) => panic!("unsafe recovery state was admitted"),
                Err(error) => error,
            }
        }
    }

    fn assert_permission_error(error: &anyhow::Error, path: &Path, bits: u32, fix: &str) {
        let text = error.to_string();
        let command = format!(
            "{fix} {}",
            crate::commands::cleanup::recovery_path::shell_word(path).unwrap()
        );
        for part in [path.display().to_string(), format!("{bits:04o}"), command] {
            assert!(
                text.contains(&part),
                "missing {part:?} in top-level error: {text}"
            );
        }
        assert_eq!(mode(path), bits, "diagnostics changed the failing object");
    }

    #[test]
    fn admission_names_the_actual_nonprivate_directory() {
        for (name, bits) in [
            ("runs", 0o775),
            ("run", 0o755),
            ("recovery", 0o755),
            ("recovery-index", 0o755),
            ("identity", 0o755),
        ] {
            let f = Fixture::new();
            let path = match name {
                "runs" => f.root.join("runs"),
                "run" => f.run().to_path_buf(),
                "recovery" => f.run().join("recovery"),
                "recovery-index" => f.root.join("recovery-index"),
                "identity" => f.index(),
                _ => unreachable!(),
            };
            private_dir(&path);
            fs::set_permissions(&path, fs::Permissions::from_mode(bits)).unwrap();
            assert_permission_error(&f.error(), &path, bits, "chmod 700");
        }
    }

    #[test]
    fn admission_names_nonprivate_lock_and_latest_document_without_exposing_contents() {
        for name in ["owner.lock", "admission.lock", "latest.json"] {
            let f = Fixture::new();
            let parent = if name == "owner.lock" {
                f.run().join("recovery")
            } else {
                f.index()
            };
            private_dir(&parent);
            let path = parent.join(name);
            fs::write(&path, b"private-document-must-not-appear-in-diagnostics").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            let error = f.error();
            assert_permission_error(&error, &path, 0o640, "chmod 600");
            assert!(
                !error
                    .to_string()
                    .contains("private-document-must-not-appear")
            );
        }
    }

    #[test]
    fn admission_keeps_the_original_runner_error_as_a_cause() {
        let f = Fixture::new();
        let path = f.root.join("recovery-index");
        private_dir(&path);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let error = f.error();
        assert!(
            error.chain().skip(1).any(|cause| cause
                .to_string()
                .contains("recovery directory must be owned and mode 0700")),
            "runner cause was flattened or lost: {error:#}"
        );
    }

    #[test]
    fn admission_diagnostics_stop_at_a_symlink_without_inspecting_its_target() {
        let f = Fixture::new();
        let elsewhere = f.root.parent().unwrap().join("outside");
        let hidden = elsewhere.join("hidden-child");
        private_dir(&hidden);
        fs::set_permissions(&hidden, fs::Permissions::from_mode(0o775)).unwrap();
        let link = f.run().join("recovery");
        symlink(&elsewhere, &link).unwrap();
        let error = f.error();
        assert!(
            error.to_string().contains(&link.display().to_string()),
            "{error:#}"
        );
        assert!(!error.to_string().contains("hidden-child"), "{error:#}");
        assert_eq!(mode(&hidden), 0o775);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            !elsewhere.join("owner.lock").exists(),
            "followed recovery symlink"
        );
    }

    #[test]
    fn admission_names_a_file_where_a_recovery_directory_is_required() {
        let f = Fixture::new();
        let path = f.root.join("recovery-index");
        fs::write(&path, b"keep this file").unwrap();
        let error = f.error();
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "{error:#}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"keep this file");
    }

    #[test]
    fn unsafe_sibling_identities_and_historical_runs_do_not_change_admission() {
        let f = Fixture::new();
        let other_index = f.root.join("recovery-index/unrelated-identity");
        let other_run = f.root.join("runs/unrelated-run/recovery");
        for path in [&other_index, &other_run] {
            private_dir(path);
            fs::set_permissions(path, fs::Permissions::from_mode(0o775)).unwrap();
        }
        f.admit().unwrap();
        assert_eq!(mode(&other_index), 0o775);
        assert_eq!(mode(&other_run), 0o775);
    }
}
