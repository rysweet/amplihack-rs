//! Orders an installed `recipe-runner-rs` revision against the pinned one, so
//! the installers never replace a runner with an older revision (issue #1593).
//!
//! [`classify`] answers `Unknown` or `Equal` without running git. Otherwise
//! it asks Cargo's git databases (`$CARGO_HOME/git/db/amplihack-recipe-runner-*`)
//! and, if neither SHA pair is found there, fetches both SHAs into a private
//! bare cache. Every failure maps to [`RunnerOrder::Unverifiable`], which keeps
//! the installed runner, and never to `Older`.
//!
//! The guard protects against accidental regression only. Integrity still
//! rests on the pinned SHA, HTTPS and `cargo install --locked --rev`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Setting this to exactly `1` lets an installer replace a runner that is
/// newer than, diverged from, or not comparable with the pin.
pub(crate) const FORCE_ENV: &str = "AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE";

/// Prefix of the directories Cargo keeps for git sources from the runner
/// repository under `$CARGO_HOME/git/db`.
const CARGO_DB_PREFIX: &str = "amplihack-recipe-runner-";

/// How the installed runner revision relates to the pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RunnerOrder {
    /// No installed revision could be read from the Cargo receipt.
    Unknown,
    /// The installed revision is the pin.
    Equal,
    /// The installed revision is a strict ancestor of the pin.
    Older,
    /// The installed revision is a strict descendant of the pin.
    Newer,
    /// Neither revision is an ancestor of the other.
    Diverged,
    /// The order could not be established.
    Unverifiable,
}

impl RunnerOrder {
    /// Whether an installer keeps the installed runner unless forced.
    pub(crate) fn keeps_installed(self) -> bool {
        matches!(self, Self::Newer | Self::Diverged | Self::Unverifiable)
    }
}

/// Where ancestry is looked up.
#[derive(Debug, Clone)]
pub(crate) enum AncestrySource {
    /// Cargo's git databases under `cargo_home`, then a fetch from `url`
    /// (HTTPS only) into the bare repository `cache`.
    Default {
        cargo_home: PathBuf,
        cache: PathBuf,
        url: String,
    },
    /// Only this local repository. Nothing is fetched. Production code always
    /// uses `Default`; tests inject fixtures through this variant.
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(PathBuf),
}

/// True only when [`FORCE_ENV`] is exactly `1`.
pub(crate) fn downgrade_forced() -> bool {
    std::env::var_os(FORCE_ENV).is_some_and(|value| value == "1")
}

/// A full lowercase hexadecimal commit SHA.
pub(crate) fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The warning every installer gives when it keeps the installed runner.
pub(crate) fn skip_message(installed: &str, pin: &str, class: RunnerOrder) -> String {
    match class {
        RunnerOrder::Newer => format!(
            "kept recipe-runner-rs {installed} (newer than pinned {pin}); set {FORCE_ENV}=1 to downgrade"
        ),
        RunnerOrder::Diverged => format!(
            "kept recipe-runner-rs {installed} (diverged from pinned {pin}); set {FORCE_ENV}=1 to replace it"
        ),
        _ => format!(
            "kept recipe-runner-rs {installed} (could not verify order against pinned {pin}); set {FORCE_ENV}=1 to replace it"
        ),
    }
}

/// Order `installed` against `pin`.
pub(crate) fn classify(installed: Option<&str>, pin: &str, source: &AncestrySource) -> RunnerOrder {
    let Some(installed) = installed else {
        return RunnerOrder::Unknown;
    };
    if !is_sha(installed) || !is_sha(pin) {
        return RunnerOrder::Unverifiable;
    }
    if installed == pin {
        return RunnerOrder::Equal;
    }
    match source {
        AncestrySource::Fixed(repo) => {
            relation(repo, installed, pin, true).unwrap_or(RunnerOrder::Unverifiable)
        }
        AncestrySource::Default {
            cargo_home,
            cache,
            url,
        } => cargo_databases(cargo_home)
            .iter()
            .find_map(|db| relation(db, installed, pin, false))
            .unwrap_or_else(|| from_cache(cache, url, installed, pin)),
    }
}

/// Bare repositories Cargo keeps for the runner repository.
fn cargo_databases(cargo_home: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(cargo_home.join("git").join("db")) else {
        return Vec::new();
    };
    let mut dbs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(CARGO_DB_PREFIX))
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dbs.sort();
    dbs
}

/// A git command against the repository at `repo` with every setting that
/// could run code, prompt or reach an unexpected transport disabled.
fn hardened_git(repo: &Path, allow_file: bool) -> Command {
    let git_dir = if repo.join(".git").is_dir() {
        repo.join(".git")
    } else {
        repo.to_path_buf()
    };
    let mut command = amplihack_git::command();
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .arg("-c")
        .arg("protocol.allow=never")
        .arg("-c")
        .arg("protocol.https.allow=always")
        .arg("-c")
        .arg(if allow_file {
            "protocol.file.allow=always"
        } else {
            "protocol.file.allow=never"
        })
        .arg("-c")
        .arg("credential.helper=")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("http.lowSpeedLimit=1000")
        .arg("-c")
        .arg("http.lowSpeedTime=30")
        .arg(format!("--git-dir={}", git_dir.display()));
    command
}

/// Run a hardened git command and return its exit code, or `None` if it
/// could not be spawned or was killed.
fn git_status(repo: &Path, allow_file: bool, args: &[&str]) -> Option<i32> {
    let output = hardened_git(repo, allow_file).args(args).output();
    match output {
        Ok(output) => output.status.code(),
        Err(err) => {
            tracing::debug!(%err, repo = %repo.display(), "git could not be run for runner ancestry");
            None
        }
    }
}

fn has_commit(repo: &Path, allow_file: bool, sha: &str) -> bool {
    let object = format!("{sha}^{{commit}}");
    git_status(repo, allow_file, &["cat-file", "-e", &object]) == Some(0)
}

/// The order of `installed` against `pin` in `repo`, or `None` when the
/// repository cannot answer (unreadable, or missing either commit).
fn relation(repo: &Path, installed: &str, pin: &str, allow_file: bool) -> Option<RunnerOrder> {
    if !repo.is_dir()
        || !has_commit(repo, allow_file, installed)
        || !has_commit(repo, allow_file, pin)
    {
        return None;
    }
    let is_ancestor = |a: &str, b: &str| match git_status(
        repo,
        allow_file,
        &["merge-base", "--is-ancestor", a, b],
    ) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    };
    if is_ancestor(installed, pin)? {
        return Some(RunnerOrder::Older);
    }
    if is_ancestor(pin, installed)? {
        return Some(RunnerOrder::Newer);
    }
    Some(RunnerOrder::Diverged)
}

/// Answer from the private cache, fetching the two SHAs into it first when
/// it cannot answer yet.
fn from_cache(cache: &Path, url: &str, installed: &str, pin: &str) -> RunnerOrder {
    if !url.starts_with("https://") {
        tracing::debug!(url = %redact_url(url), "runner ancestry source is not an HTTPS URL");
        return RunnerOrder::Unverifiable;
    }
    if let Err(reason) = prepare_cache(cache) {
        tracing::warn!(cache = %cache.display(), "runner ancestry cache unusable: {reason}");
        return RunnerOrder::Unverifiable;
    }
    if let Some(order) = relation(cache, installed, pin, false) {
        return order;
    }
    let fetched = hardened_git(cache, false)
        .args([
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--filter=blob:none",
            "--",
            url,
            installed,
            pin,
        ])
        .output();
    match fetched {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            tracing::debug!(
                url = %redact_url(url),
                status = %output.status,
                "runner ancestry fetch failed"
            );
            return RunnerOrder::Unverifiable;
        }
        Err(err) => {
            tracing::debug!(%err, "runner ancestry fetch could not be run");
            return RunnerOrder::Unverifiable;
        }
    }
    relation(cache, installed, pin, false).unwrap_or(RunnerOrder::Unverifiable)
}

/// Create `cache` as a private bare repository, or confirm that an existing
/// `cache` is one. An existing path that is anything else is never touched.
fn prepare_cache(cache: &Path) -> Result<(), String> {
    let empty_dir = std::fs::read_dir(cache).is_ok_and(|mut entries| entries.next().is_none());
    if cache.exists() && !empty_dir {
        let output = hardened_git(cache, false)
            .args(["rev-parse", "--is-bare-repository"])
            .output()
            .map_err(|err| format!("git could not be run: {err}"))?;
        if output.status.success() && output.stdout.trim_ascii() == b"true" {
            return Ok(());
        }
        return Err("the path exists and is not a bare git repository".to_string());
    }
    if !cache.exists() {
        if let Some(parent) = cache.parent() {
            create_private_dir_all(parent)
                .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
        }
        create_private_dir(cache).map_err(|err| format!("cannot create the cache: {err}"))?;
    }
    let status = amplihack_git::command()
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "init",
            "--quiet",
            "--bare",
        ])
        .arg(cache)
        .output()
        .map_err(|err| format!("git init could not be run: {err}"))?
        .status;
    if !status.success() {
        return Err(format!("git init --bare exited with {status}"));
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(unix)]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

/// `url` with any `user:password@` removed, for logging.
pub(crate) fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.chars().take(64).collect();
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    let rest = match rest[..host_end].rfind('@') {
        Some(at) => &rest[at + 1..],
        None => rest,
    };
    format!("{scheme}://{rest}")
}

/// Fixture runner repositories shared by this module's tests and the
/// freshness worker tests.
#[cfg(test)]
pub(crate) mod fixture {
    use std::path::{Path, PathBuf};

    /// A runner repository with history `A - B - C` on `main` and `D` on a
    /// side branch from `A`.
    pub(crate) struct Runner {
        pub(crate) path: PathBuf,
        pub(crate) a: String,
        pub(crate) b: String,
        pub(crate) c: String,
        pub(crate) d: String,
    }

    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let output = amplihack_git::command_in(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Builds the fixture under `root`. Holds the crate's environment lock
    /// while it runs git, because other tests point `PATH` elsewhere.
    pub(crate) fn build(root: &Path) -> Runner {
        let _env = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let path = root.join("runner-fixture");
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["config", "uploadpack.allowFilter", "true"]);
        git(&path, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        let commit = |message: &str| {
            git(&path, &["commit", "-q", "--allow-empty", "-m", message]);
            git(&path, &["rev-parse", "HEAD"])
        };
        let a = commit("A");
        let b = commit("B");
        let c = commit("C");
        git(&path, &["checkout", "-q", "-b", "side", &a]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "D"]);
        let d = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["checkout", "-q", "main"]);
        Runner { path, a, b, c, d }
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{self, Runner};
    use super::*;
    use std::path::{Path, PathBuf};

    const MISSING: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A scratch directory that also holds the environment lock, so that no
    /// other test changes `PATH` while `classify` runs git.
    struct Scratch {
        dir: tempfile::TempDir,
        _env: std::sync::MutexGuard<'static, ()>,
    }

    impl Scratch {
        fn path(&self) -> &Path {
            self.dir.path()
        }
    }

    fn runner() -> (Scratch, Runner) {
        let dir = tempfile::tempdir().unwrap();
        let runner = fixture::build(dir.path());
        let env = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        (Scratch { dir, _env: env }, runner)
    }

    fn fixed(runner: &Runner) -> AncestrySource {
        AncestrySource::Fixed(runner.path.clone())
    }

    #[test]
    fn classifies_all_six_orders_against_a_fixture() {
        let (_dir, r) = runner();
        let source = fixed(&r);
        assert_eq!(classify(None, &r.b, &source), RunnerOrder::Unknown);
        assert_eq!(classify(Some(&r.b), &r.b, &source), RunnerOrder::Equal);
        assert_eq!(classify(Some(&r.a), &r.b, &source), RunnerOrder::Older);
        assert_eq!(classify(Some(&r.c), &r.b, &source), RunnerOrder::Newer);
        assert_eq!(classify(Some(&r.d), &r.b, &source), RunnerOrder::Diverged);
        assert_eq!(
            classify(Some(MISSING), &r.b, &source),
            RunnerOrder::Unverifiable
        );
    }

    #[test]
    fn unknown_and_equal_need_no_git() {
        // A source that cannot be read at all still answers these two.
        let source = AncestrySource::Fixed(PathBuf::from("/nonexistent/runner-fixture"));
        let pin = "4d5187eaf95836a549e6e68725f8f853d0dc7034";
        assert_eq!(classify(None, pin, &source), RunnerOrder::Unknown);
        assert_eq!(classify(Some(pin), pin, &source), RunnerOrder::Equal);
    }

    #[test]
    fn a_deleted_fixture_is_unverifiable_never_older() {
        let (dir, r) = runner();
        let source = fixed(&r);
        std::fs::remove_dir_all(&r.path).unwrap();
        for installed in [&r.a, &r.c, &r.d] {
            assert_eq!(
                classify(Some(installed), &r.b, &source),
                RunnerOrder::Unverifiable,
                "{installed}"
            );
        }
        drop(dir);
    }

    #[test]
    fn invalid_shas_are_unverifiable_and_reach_no_git() {
        let (_dir, r) = runner();
        let source = fixed(&r);
        let upper = r.c.to_uppercase();
        for bad in [
            "--upload-pack=touch /tmp/runner-ancestry-pwned",
            &r.c[..39],
            &upper,
            "HEAD",
            "main",
        ] {
            assert_eq!(
                classify(Some(bad), &r.b, &source),
                RunnerOrder::Unverifiable,
                "{bad}"
            );
        }
        assert!(!Path::new("/tmp/runner-ancestry-pwned").exists());
        // An invalid pin is never ordered either.
        assert_eq!(
            classify(Some(&r.c), "main", &source),
            RunnerOrder::Unverifiable
        );
    }

    #[test]
    fn cargo_git_db_decides_without_a_fetch() {
        let (dir, r) = runner();
        let cargo_home = dir.path().join("cargo");
        let db = cargo_home.join("git/db/amplihack-recipe-runner-0123abcd");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        fixture::git(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                r.path.to_str().unwrap(),
                db.to_str().unwrap(),
            ],
        );
        let cache = dir.path().join("state/recipe-runner.git");
        let source = AncestrySource::Default {
            cargo_home,
            cache: cache.clone(),
            // Never contacted: the Cargo database already holds both SHAs.
            url: "https://invalid.invalid/amplihack-recipe-runner".to_string(),
        };
        assert_eq!(classify(Some(&r.c), &r.b, &source), RunnerOrder::Newer);
        assert_eq!(classify(Some(&r.a), &r.b, &source), RunnerOrder::Older);
        assert_eq!(classify(Some(&r.d), &r.b, &source), RunnerOrder::Diverged);
        assert!(!cache.exists(), "no fetch was needed");
    }

    #[test]
    fn default_source_refuses_non_https_urls() {
        let (dir, r) = runner();
        let source = AncestrySource::Default {
            cargo_home: dir.path().join("empty-cargo"),
            cache: dir.path().join("state/recipe-runner.git"),
            url: format!("file://{}", r.path.display()),
        };
        assert_eq!(
            classify(Some(&r.c), &r.b, &source),
            RunnerOrder::Unverifiable
        );
        let marker = dir.path().join("ext-ran");
        let source = AncestrySource::Default {
            cargo_home: dir.path().join("empty-cargo"),
            cache: dir.path().join("state/recipe-runner.git"),
            url: format!("ext::sh -c touch% {}", marker.display()),
        };
        assert_eq!(
            classify(Some(&r.c), &r.b, &source),
            RunnerOrder::Unverifiable
        );
        assert!(!marker.exists(), "ext:: transport must never run");
    }

    #[test]
    fn a_cache_that_is_not_a_bare_repository_is_left_alone() {
        let (dir, r) = runner();
        let cache = dir.path().join("state/recipe-runner.git");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("precious"), "user data").unwrap();
        let source = AncestrySource::Default {
            cargo_home: dir.path().join("empty-cargo"),
            cache: cache.clone(),
            url: "https://invalid.invalid/amplihack-recipe-runner".to_string(),
        };
        assert_eq!(
            classify(Some(&r.c), &r.b, &source),
            RunnerOrder::Unverifiable
        );
        assert_eq!(
            std::fs::read_to_string(cache.join("precious")).unwrap(),
            "user data"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_created_cache_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, r) = runner();
        let cache = dir.path().join("state/recipe-runner.git");
        let source = AncestrySource::Default {
            cargo_home: dir.path().join("empty-cargo"),
            cache: cache.clone(),
            // Unreachable: the fetch fails, but the cache is created first.
            url: "https://127.0.0.1:9/amplihack-recipe-runner".to_string(),
        };
        assert_eq!(
            classify(Some(&r.c), &r.b, &source),
            RunnerOrder::Unverifiable
        );
        assert!(cache.exists(), "the cache is created before the fetch");
        let mode = std::fs::metadata(&cache).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "cache mode {mode:o}");
    }

    #[test]
    fn order_serialises_by_variant_name() {
        for (order, name) in [
            (RunnerOrder::Unknown, "\"Unknown\""),
            (RunnerOrder::Equal, "\"Equal\""),
            (RunnerOrder::Older, "\"Older\""),
            (RunnerOrder::Newer, "\"Newer\""),
            (RunnerOrder::Diverged, "\"Diverged\""),
            (RunnerOrder::Unverifiable, "\"Unverifiable\""),
        ] {
            assert_eq!(serde_json::to_string(&order).unwrap(), name);
            assert_eq!(serde_json::from_str::<RunnerOrder>(name).unwrap(), order);
        }
        assert!(serde_json::from_str::<RunnerOrder>("\"Bogus\"").is_err());
    }

    #[test]
    fn logged_urls_drop_userinfo() {
        assert_eq!(
            redact_url("https://user:secret@github.com/rysweet/amplihack-recipe-runner"),
            "https://github.com/rysweet/amplihack-recipe-runner"
        );
        assert_eq!(
            redact_url("https://github.com/a@b"),
            "https://github.com/a@b"
        );
    }

    #[test]
    fn downgrade_is_forced_only_by_exactly_one() {
        assert_eq!(FORCE_ENV, "AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE");
        for (value, forced) in [
            (Some("1"), true),
            (Some("01"), false),
            (Some("true"), false),
            (Some("yes"), false),
            (Some(""), false),
            (Some(" 1"), false),
            (Some("1 "), false),
            (Some("0"), false),
            (None, false),
        ] {
            let got = crate::test_support::with_env(
                &[(FORCE_ENV, value.map(std::ffi::OsStr::new))],
                downgrade_forced,
            );
            assert_eq!(got, forced, "{value:?}");
        }
    }

    /// The pre-#1593 name was never released and has no alias.
    #[test]
    fn old_force_variable_name_does_not_force() {
        let got = crate::test_support::with_env(
            &[
                (FORCE_ENV, None),
                (
                    // Split so a repository grep for the old name finds nothing.
                    concat!("AMPLIHACK_ALLOW_", "RUNNER_DOWNGRADE"),
                    Some(std::ffi::OsStr::new("1")),
                ),
            ],
            downgrade_forced,
        );
        assert!(!got, "the old variable name must not force a downgrade");
    }

    #[test]
    fn skip_messages_name_the_force_variable() {
        let installed = "c".repeat(40);
        let pin = "b".repeat(40);
        for class in [
            RunnerOrder::Newer,
            RunnerOrder::Diverged,
            RunnerOrder::Unverifiable,
        ] {
            let message = skip_message(&installed, &pin, class);
            assert!(
                message.contains("AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1"),
                "{class:?}: {message}"
            );
            assert!(
                !message.contains(concat!("ALLOW_", "RUNNER_DOWNGRADE")),
                "{message}"
            );
            assert!(message.contains(&installed), "{message}");
            assert!(message.contains(&pin), "{message}");
        }
    }
}
