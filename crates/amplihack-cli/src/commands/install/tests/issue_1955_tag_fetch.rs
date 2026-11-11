//! Issue #1955: the release tag fetch. Fake `git` scripts and an injected HTTP
//! function stand in for the network, and the one real-git test fetches from a
//! local repository through a `url.insteadOf` in its own HOME.

use super::super::clone::which_git_in;
use super::super::filesystem::copy_dir_recursive;
use super::super::framework_source::read_fetched_tree_version;
use super::super::tag_fetch::*;
use super::issue_1955_support::*;
use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

const REPO_URL: &str = "https://github.com/rysweet/amplihack-rs";
const TARBALL_URL: &str =
    "https://github.com/rysweet/amplihack-rs/archive/refs/tags/v1.2.99.tar.gz";

fn version() -> semver::Version {
    semver::Version::parse("1.2.99").unwrap()
}

const LONG: Duration = Duration::from_secs(30);

/// A `git` that logs its arguments and environment, then runs `then`.
fn recording_git(dir: &Path, then: &str) -> PathBuf {
    let path = dir.join("bin/git");
    write_script(
        &path,
        &format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{log}'\n\
             env >> '{env}'\n\
             readlink /proc/self/fd/0 >> '{stdin}'\n\
             {then}\n",
            log = dir.join("argv.log").display(),
            env = dir.join("env.log").display(),
            stdin = dir.join("stdin.log").display(),
        ),
    );
    path
}

fn log(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap_or_default()
}

/// Records each URL and budget it's asked for, then answers with `body`.
struct RecordingHttp {
    calls: Rc<RefCell<Vec<(String, Duration)>>>,
}

fn recording_http(
    body: impl Fn() -> anyhow::Result<Vec<u8>> + 'static,
) -> (RecordingHttp, Box<BoundedHttpGet>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let record = Rc::clone(&calls);
    (
        RecordingHttp { calls },
        Box::new(move |url: &str, budget: Duration| {
            record.borrow_mut().push((url.to_string(), budget));
            body()
        }),
    )
}

fn refused_http() -> (RecordingHttp, Box<BoundedHttpGet>) {
    recording_http(|| anyhow::bail!("offline"))
}

fn expect_fetch_failed(result: anyhow::Result<PathBuf>) -> String {
    let err = result.expect_err("the fetch must fail");
    match err.downcast_ref::<SourceFetchFailed>() {
        Some(SourceFetchFailed::Failed { reason, .. }) => reason.clone(),
        other => panic!("expected SourceFetchFailed::Failed, got {other:?}: {err:#}"),
    }
}

// ── D2: when a build may fetch ──

#[test]
fn only_a_release_build_may_fetch() {
    // (version, AMPLIHACK_RELEASE_VERSION set at build time, may fetch)
    let cases = [
        ("1.2.99", true, true),
        ("1.2.99-rc.1", true, true),
        // A dev build: `v1.2.0` is a real tag, and staging it is the bug.
        ("1.2.0", false, false),
        // A snapshot build, even though the release env var is set.
        ("1.2.0+snapshot.7deb23da4", true, false),
        ("1.2.0+snapshot.7deb23da4", false, false),
        ("not-semver", true, false),
    ];
    for (version, release_env_set, expected) in cases {
        assert_eq!(
            fetch_allowed(version, release_env_set),
            expected,
            "{version} release_env_set={release_env_set}"
        );
    }
}

// ── The git route ──

#[test]
fn git_fetches_exactly_the_tag_from_the_fixed_url() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    // The fake makes no tree, so the result doesn't matter: its argv does.
    let _ = fetcher.fetch(&version(), &temp.path().join("dest"), LONG);

    let argv = log(temp.path(), "argv.log");
    let fetch = argv
        .lines()
        .find(|line| line.contains(" fetch ") || line.starts_with("fetch "))
        .unwrap_or_else(|| panic!("no `git fetch` was run:\n{argv}"));
    assert!(fetch.contains("--depth 1"), "{fetch}");
    let tag = "refs/tags/v1.2.99";
    assert!(fetch.contains(tag), "{fetch}");
    // SEC-7: `--` comes before the positional URL and refspec.
    assert!(fetch.contains(&format!("-- {REPO_URL} {tag}")), "{fetch}");
}

#[test]
fn git_never_clones_or_touches_a_branch_or_a_remote() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let _ = fetcher.fetch(&version(), &temp.path().join("dest"), LONG);

    let argv = log(temp.path(), "argv.log");
    assert!(!argv.is_empty(), "git was not run");
    for forbidden in ["clone", "refs/heads", " main", "remote add", "origin"] {
        assert!(!argv.contains(forbidden), "{forbidden:?} in:\n{argv}");
    }
}

#[test]
fn every_git_call_turns_off_hooks_fsmonitor_and_credential_helpers() {
    // SEC-6.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let _ = fetcher.fetch(&version(), &temp.path().join("dest"), LONG);

    let argv = log(temp.path(), "argv.log");
    assert!(!argv.is_empty(), "git was not run");
    for line in argv.lines() {
        for setting in [
            "-c core.hooksPath=/dev/null",
            "-c core.fsmonitor=false",
            "-c credential.helper=",
        ] {
            assert!(line.contains(setting), "{setting:?} missing from: {line}");
        }
    }
}

#[test]
fn inherited_git_location_variables_do_not_reach_git() {
    // SEC-2: all 15 from `git rev-parse --local-env-vars`, plus the five that
    // can run a command or point at templates.
    const SCRUBBED: [&str; 20] = [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
        "GIT_OBJECT_DIRECTORY",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_GRAFT_FILE",
        "GIT_INDEX_FILE",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_REPLACE_REF_BASE",
        "GIT_PREFIX",
        "GIT_SHALLOW_FILE",
        "GIT_COMMON_DIR",
        "GIT_SSH_COMMAND",
        "GIT_PROXY_COMMAND",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_TEMPLATE_DIR",
    ];
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let hostile = std::ffi::OsStr::new("/hostile/value");
    let vars: Vec<(&str, Option<&std::ffi::OsStr>)> =
        SCRUBBED.iter().map(|name| (*name, Some(hostile))).collect();

    crate::test_support::with_env(&vars, || {
        let (_, http) = refused_http();
        let fetcher = ReleaseTagFetcher {
            git: Some(git),
            http,
        };
        let _ = fetcher.fetch(&version(), &temp.path().join("dest"), LONG);
    });

    let env = log(temp.path(), "env.log");
    assert!(!env.is_empty(), "git was not run");
    for name in SCRUBBED {
        assert!(
            !env.lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} reached git"
        );
    }
    assert!(env.contains("GIT_TERMINAL_PROMPT=0"), "prompts must be off");
}

#[test]
fn git_runs_with_no_stdin_and_in_its_own_session() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let sessions = temp.path().join("sessions.log");
    let git = recording_git(
        temp.path(),
        &format!(
            "echo \"$$ $(cut -d' ' -f6 /proc/$$/stat)\" >> '{}'\nexit 0",
            sessions.display()
        ),
    );
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let _ = fetcher.fetch(&version(), &temp.path().join("dest"), LONG);

    let stdin = log(temp.path(), "stdin.log");
    assert!(!stdin.is_empty(), "git was not run");
    assert!(stdin.lines().all(|line| line == "/dev/null"), "{stdin}");
    for line in fs::read_to_string(&sessions).unwrap().lines() {
        let (pid, session) = line.split_once(' ').unwrap();
        assert_eq!(
            pid, session,
            "git must lead its own session so a kill reaches it"
        );
    }
}

/// The real thing, offline. A branch with the tag's name is what makes `git
/// clone --branch` the wrong tool (D7).
#[test]
fn the_real_git_route_gets_the_tag_even_when_a_branch_has_the_same_name() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let upstream = temp.path().join("upstream");
    // Read under the lock `InstallEnv` holds while it changes PATH.
    let git_path = {
        let _path = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        which_git_in(&std::env::var_os("PATH").unwrap()).unwrap()
    };
    // Spawn under the same lock: tests in this module set hostile `GIT_*`
    // values (`GIT_COMMON_DIR` among them) process-wide via `with_env`, and a
    // child spawned meanwhile would inherit them.
    let git = |dir: &Path, args: &[&str]| {
        let _env = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let output = Command::new(&git_path)
            .args(args)
            .current_dir(dir)
            .env("HOME", temp.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--quiet"]);
    versioned_tree(&upstream, "1.2.99");
    git(&upstream, &["add", "-A"]);
    git(
        &upstream,
        &["commit", "--quiet", "--no-gpg-sign", "-m", "tag"],
    );
    git(&upstream, &["tag", "v1.2.99"]);
    write_workspace_toml(&upstream, "9.9.9");
    git(
        &upstream,
        &["commit", "--quiet", "--no-gpg-sign", "-am", "branch"],
    );
    git(&upstream, &["branch", "v1.2.99"]);
    fs::write(
        temp.path().join(".gitconfig"),
        format!(
            "[url \"file://{}\"]\n\tinsteadOf = {REPO_URL}\n",
            upstream.display()
        ),
    )
    .unwrap();
    let (http_calls, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git_path.clone()),
        http,
    };
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();

    let root = crate::test_support::with_env(
        &[
            ("HOME", Some(temp.path().as_os_str())),
            ("GIT_CONFIG_NOSYSTEM", Some(std::ffi::OsStr::new("1"))),
        ],
        || fetcher.fetch(&version(), &dest, LONG),
    )
    .unwrap_or_else(|e| panic!("{e:#}"));

    assert!(
        http_calls.calls.borrow().is_empty(),
        "the tarball route must not be needed"
    );
    assert_eq!(
        read_fetched_tree_version(&root).to_string(),
        "v1.2.99",
        "the tag's tree, not the branch's"
    );
    assert!(root.join("amplifier-bundle/recipes").is_dir());
}

// ── Falling back to the tarball ──

/// A tag tarball as GitHub makes it: one top-level directory, with symlinks.
fn tag_tarball(version: &str) -> Vec<u8> {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let tree = temp.path().join(format!("amplihack-rs-{version}"));
    versioned_tree(&tree, version);
    std::os::unix::fs::symlink(
        "marker.txt",
        tree.join("amplifier-bundle/agents/link-to-marker"),
    )
    .unwrap();
    std::os::unix::fs::symlink("agents", tree.join("amplifier-bundle/link-to-agents")).unwrap();
    gz_of(|builder| {
        builder
            .append_dir_all(tree.file_name().unwrap(), &tree)
            .unwrap();
    })
}

fn gz_of(fill: impl FnOnce(&mut tar::Builder<flate2::write::GzEncoder<&mut Vec<u8>>>)) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        builder.follow_symlinks(false);
        fill(&mut builder);
        builder.into_inner().unwrap().finish().unwrap();
    }
    bytes
}

#[test]
fn when_git_fails_the_exact_tag_tarball_is_downloaded() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 1");
    let (calls, http) = recording_http(|| Ok(tag_tarball("1.2.99")));
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();

    let root = fetcher
        .fetch(&version(), &dest, LONG)
        .unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(calls.calls.borrow().len(), 1);
    assert_eq!(calls.calls.borrow()[0].0, TARBALL_URL);
    assert!(root.join("amplifier-bundle/recipes").is_dir(), "{root:?}");
    assert_eq!(read_fetched_tree_version(&root).to_string(), "v1.2.99");
}

#[test]
fn with_no_git_the_tarball_is_the_only_route() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let (calls, http) = recording_http(|| Ok(tag_tarball("1.2.99")));
    let fetcher = ReleaseTagFetcher { git: None, http };
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();

    fetcher
        .fetch(&version(), &dest, LONG)
        .unwrap_or_else(|e| panic!("{e:#}"));

    assert_eq!(calls.calls.borrow()[0].0, TARBALL_URL);
}

#[test]
fn a_partial_git_tree_is_not_left_for_the_tarball_to_unpack_into() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let git = recording_git(
        temp.path(),
        &format!(
            "mkdir -p '{}'/stale/amplifier-bundle\nexit 1",
            dest.display()
        ),
    );
    let (_, http) = recording_http(|| Ok(tag_tarball("1.2.99")));
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let root = fetcher
        .fetch(&version(), &dest, LONG)
        .unwrap_or_else(|e| panic!("{e:#}"));

    assert!(
        root.ends_with("amplihack-rs-1.2.99"),
        "the tree from the failed route must not be chosen: {root:?}"
    );
}

#[test]
fn the_budget_is_shared_across_both_routes() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "/bin/sleep 1\nexit 1");
    let (calls, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };
    let budget = Duration::from_secs(20);

    let _ = fetcher.fetch(&version(), &temp.path().join("dest"), budget);

    let calls = calls.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].1 < budget - Duration::from_millis(500),
        "the tarball must get what git left, got {:?}",
        calls[0].1
    );
}

#[test]
fn a_hung_git_is_killed_with_its_children_when_the_budget_runs_out() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let marker = temp.path().join("orphan-marker");
    let git = recording_git(
        temp.path(),
        &format!(
            "(/bin/sleep 4; echo orphan > '{}') &\nexec /bin/sleep 60",
            marker.display()
        ),
    );
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };
    let started = Instant::now();

    let reason = expect_fetch_failed(fetcher.fetch(
        &version(),
        &temp.path().join("dest"),
        Duration::from_millis(500),
    ));

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the budget must bound the wait, took {:?}: {reason}",
        started.elapsed()
    );
    std::thread::sleep(Duration::from_secs(5).saturating_sub(started.elapsed()));
    assert!(!marker.exists(), "git's children must be killed too");
}

#[test]
fn both_routes_failing_is_a_source_fetch_failure() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 1");
    let (_, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    expect_fetch_failed(fetcher.fetch(&version(), &temp.path().join("dest"), LONG));
}

#[test]
fn the_failure_reason_is_one_line_with_no_captured_output() {
    // SEC-14: the reason ends up in a one-line warning on every launch.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(
        temp.path(),
        "printf 'fatal\\033[2J\\nsecond line\\n' >&2\nexit 1",
    );
    let (_, http) = recording_http(|| anyhow::bail!("offline\nsecond\u{1b}[2J"));
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };

    let reason = expect_fetch_failed(fetcher.fetch(&version(), &temp.path().join("dest"), LONG));

    assert!(!reason.is_empty());
    assert!(!reason.chars().any(char::is_control), "{reason:?}");
}

#[test]
fn a_version_with_build_metadata_is_rejected_before_any_route_runs() {
    // SEC-7: checked inside the fetcher as well as in D2.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exit 0");
    let (calls, http) = refused_http();
    let fetcher = ReleaseTagFetcher {
        git: Some(git),
        http,
    };
    let snapshot = semver::Version::parse("1.2.0+snapshot.7deb23da4").unwrap();

    let err = fetcher
        .fetch(&snapshot, &temp.path().join("dest"), LONG)
        .expect_err("must be rejected");

    assert!(
        matches!(
            err.downcast_ref::<SourceFetchFailed>(),
            Some(SourceFetchFailed::NotAllowed { .. })
        ),
        "{err:#}"
    );
    assert_eq!(log(temp.path(), "argv.log"), "", "git must not run");
    assert!(calls.calls.borrow().is_empty(), "no download either");
}

// ── Finding git (SEC-8) ──

#[test]
fn a_relative_path_entry_is_not_searched_for_git() {
    // Version reading runs in untrusted directories, so `.` on PATH must not
    // hand git to whatever the directory holds.
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_script(&temp.path().join("planted/git"), "#!/bin/sh\nexit 0\n");
    write_script(&temp.path().join("trusted/git"), "#!/bin/sh\nexit 0\n");
    let _lock = crate::test_support::cwd_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cwd = crate::test_support::CwdGuard::set(temp.path()).unwrap();
    let trusted = temp.path().join("trusted");

    let path = std::env::join_paths([Path::new("planted"), Path::new(""), &trusted]).unwrap();

    assert_eq!(which_git_in(&path).unwrap(), trusted.join("git"));
}

#[test]
fn only_relative_entries_means_no_git() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    write_script(&temp.path().join("planted/git"), "#!/bin/sh\nexit 0\n");
    let _lock = crate::test_support::cwd_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _cwd = crate::test_support::CwdGuard::set(temp.path()).unwrap();

    assert!(which_git_in(std::ffi::OsStr::new("planted:.")).is_err());
}

// ── `git describe` bounds (SEC-9) ──

#[test]
fn a_hung_git_describe_gives_unknown_within_the_limit() {
    use super::super::framework_source::{SourceVersion, read_source_version_with_git};
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "exec /bin/sleep 60");
    let started = Instant::now();

    let version = read_source_version_with_git(temp.path(), Some(&git));

    assert_eq!(version, SourceVersion::Unknown);
    assert!(!log(temp.path(), "argv.log").is_empty(), "git was not run");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn oversized_git_describe_output_gives_unknown() {
    use super::super::framework_source::{SourceVersion, read_source_version_with_git};
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(
        temp.path(),
        "head -c 4096 /dev/zero | tr '\\0' '1'\nprintf '.2.3\\n'\nexit 0",
    );

    let version = read_source_version_with_git(temp.path(), Some(&git));

    assert_eq!(version, SourceVersion::Unknown);
    assert!(!log(temp.path(), "argv.log").is_empty(), "git was not run");
}

#[test]
fn git_describe_asks_for_an_exact_v_tag_with_the_same_protections() {
    use super::super::framework_source::read_source_version_with_git;
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let git = recording_git(temp.path(), "echo v1.2.99");

    let version = read_source_version_with_git(temp.path(), Some(&git));

    assert_eq!(version.to_string(), "v1.2.99");
    let argv = log(temp.path(), "argv.log");
    assert!(
        argv.contains("describe --exact-match --tags --match v*"),
        "{argv}"
    );
    assert!(argv.contains("-c core.hooksPath=/dev/null"), "{argv}");
    assert_eq!(
        log(temp.path(), "stdin.log").trim(),
        "/dev/null",
        "no prompt may wait on a tty"
    );
}

// ── The bounded extractor (SEC-3) ──

const SMALL: TarballLimits = TarballLimits {
    max_download_bytes: 64 * 1024,
    max_unpacked_bytes: 64 * 1024,
    max_entries: 16,
};

#[test]
fn the_agreed_limits_are_in_force() {
    assert_eq!(TARBALL_LIMITS.max_download_bytes, 256 * 1024 * 1024);
    assert_eq!(TARBALL_LIMITS.max_unpacked_bytes, 1024 * 1024 * 1024);
    assert_eq!(TARBALL_LIMITS.max_entries, 100_000);
}

/// One regular entry per item, named as given with no path check by the writer.
fn raw_entry(
    builder: &mut tar::Builder<flate2::write::GzEncoder<&mut Vec<u8>>>,
    name: &str,
    kind: tar::EntryType,
    mode: u32,
    link: Option<&str>,
    data: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    let gnu = header.as_gnu_mut().unwrap();
    gnu.name[..name.len()].copy_from_slice(name.as_bytes());
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(data.len() as u64);
    if let Some(link) = link {
        header.set_link_name(link).unwrap();
    }
    header.set_cksum();
    builder.append(&header, data).unwrap();
}

fn regular(
    builder: &mut tar::Builder<flate2::write::GzEncoder<&mut Vec<u8>>>,
    name: &str,
    mode: u32,
    data: &[u8],
) {
    raw_entry(builder, name, tar::EntryType::Regular, mode, None, data);
}

#[test]
fn a_clean_archive_unpacks_with_its_contents_and_execute_bit() {
    let archive = gz_of(|b| {
        regular(b, "top/file.txt", 0o664, b"data");
        regular(b, "top/run.sh", 0o775, b"#!/bin/sh\n");
    });
    let temp = crate::test_support::tempdir_outside_framework_roots();

    extract_tag_tarball(&archive, temp.path(), &SMALL).unwrap();

    assert_eq!(fs::read(temp.path().join("top/file.txt")).unwrap(), b"data");
    let mode = fs::metadata(temp.path().join("top/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(
        mode & 0o111,
        0,
        "the execute bit is what the mode rule reads"
    );
}

#[test]
fn setuid_is_not_kept() {
    let archive = gz_of(|b| regular(b, "top/tool", 0o6755, b"x"));
    let temp = crate::test_support::tempdir_outside_framework_roots();

    extract_tag_tarball(&archive, temp.path(), &SMALL).unwrap();

    let mode = fs::metadata(temp.path().join("top/tool"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o7000, 0, "{mode:o}");
}

#[test]
fn too_many_entries_fails() {
    let archive = gz_of(|b| {
        for i in 0..17 {
            regular(b, &format!("top/f{i}"), 0o644, b"");
        }
    });
    let temp = crate::test_support::tempdir_outside_framework_roots();

    let reason = extract_tag_tarball(&archive, temp.path(), &SMALL).expect_err("must fail");

    assert!(reason.contains("entries"), "{reason}");
}

#[test]
fn too_many_unpacked_bytes_fails_even_from_a_small_archive() {
    let data = vec![0u8; 65 * 1024];
    let archive = gz_of(|b| regular(b, "top/bomb", 0o644, &data));
    assert!((archive.len() as u64) < SMALL.max_download_bytes);
    let temp = crate::test_support::tempdir_outside_framework_roots();

    let reason = extract_tag_tarball(&archive, temp.path(), &SMALL).expect_err("must fail");

    assert!(reason.contains("unpacks to over"), "{reason}");
}

/// `tar` reads a metadata entry into memory before the extractor sees the next
/// entry, so neither the entry count nor the unpacked total includes it.
#[test]
fn an_oversized_metadata_entry_fails_even_from_a_small_archive() {
    // The length prefix counts the whole record: 16 + 1048559 + 1 bytes.
    let record = format!("1048576 comment={}\n", "x".repeat(1_048_559));
    let archive = gz_of(|b| {
        raw_entry(
            b,
            "top/pax",
            tar::EntryType::XHeader,
            0o644,
            None,
            record.as_bytes(),
        );
        regular(b, "top/file.txt", 0o644, b"data");
    });
    assert!((archive.len() as u64) < SMALL.max_download_bytes);
    let temp = crate::test_support::tempdir_outside_framework_roots();

    let reason = extract_tag_tarball(&archive, temp.path(), &SMALL).expect_err("must fail");

    // Cut off by the stream cap, either inside the entry or at its end.
    assert!(
        reason.contains("decompresses to over") || reason.contains("can't be read"),
        "{reason}"
    );
}

/// GitHub's own archives hold metadata entries, so a normal one must still pass.
#[test]
fn a_normal_metadata_entry_still_unpacks() {
    let archive = gz_of(|b| {
        raw_entry(
            b,
            "top/pax",
            tar::EntryType::XHeader,
            0o644,
            None,
            b"15 comment=abc\n",
        );
        regular(b, "top/file.txt", 0o644, b"data");
    });
    let temp = crate::test_support::tempdir_outside_framework_roots();

    extract_tag_tarball(&archive, temp.path(), &SMALL).unwrap();

    assert_eq!(fs::read(temp.path().join("top/file.txt")).unwrap(), b"data");
}

#[test]
fn too_many_download_bytes_fails() {
    let archive = gz_of(|b| regular(b, "top/file", 0o644, b"data"));
    let limits = TarballLimits {
        max_download_bytes: archive.len() as u64 - 1,
        ..SMALL
    };
    let temp = crate::test_support::tempdir_outside_framework_roots();

    let reason = extract_tag_tarball(&archive, temp.path(), &limits).expect_err("must fail");

    assert!(reason.contains("is over the limit"), "{reason}");
}

#[test]
fn garbage_is_a_failed_fetch_not_a_panic() {
    let temp = crate::test_support::tempdir_outside_framework_roots();

    let reason = extract_tag_tarball(b"not a tarball", temp.path(), &SMALL).expect_err("must fail");

    assert!(reason.contains("can't be read"), "{reason}");
}

/// A hostile entry may be refused or skipped. The tests check what was written.
fn extract_hostile(archive: &[u8], dest: &Path) {
    if let Err(reason) = extract_tag_tarball(archive, dest, &SMALL) {
        assert!(!reason.is_empty());
    }
}

#[test]
fn a_path_above_the_destination_writes_nothing_there() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let dest = temp.path().join("nested/dest");
    fs::create_dir_all(&dest).unwrap();
    let archive = gz_of(|b| {
        regular(b, "top/ok", 0o644, b"ok");
        regular(b, "../escaped", 0o644, b"x");
        regular(b, "top/../../escaped-too", 0o644, b"x");
    });

    extract_hostile(&archive, &dest);

    assert!(!temp.path().join("nested/escaped").exists());
    assert!(!temp.path().join("nested/escaped-too").exists());
}

#[test]
fn an_absolute_path_writes_nothing_there() {
    // The absolute name must fit the 100-byte tar name field, which a long
    // TMPDIR (recipe runs use one) would overflow.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let outside = temp.path().join("absolute-target");
    let archive = gz_of(|b| regular(b, outside.to_str().unwrap(), 0o644, b"x"));

    extract_hostile(&archive, &dest);

    assert!(!outside.exists());
}

#[test]
fn a_file_through_a_symlink_entry_writes_nothing_outside() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let archive = gz_of(|b| {
        raw_entry(
            b,
            "top/link",
            tar::EntryType::Symlink,
            0o777,
            Some(outside.to_str().unwrap()),
            b"",
        );
        regular(b, "top/link/planted", 0o644, b"x");
    });

    extract_hostile(&archive, &dest);

    assert!(!outside.join("planted").exists());
}

#[test]
fn a_hard_link_to_a_file_outside_gets_none_of_its_bytes() {
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();
    let secret = temp.path().join("secret");
    fs::write(&secret, "secret").unwrap();
    let archive = gz_of(|b| {
        raw_entry(
            b,
            "top/stolen",
            tar::EntryType::Link,
            0o644,
            Some(secret.to_str().unwrap()),
            b"",
        );
    });

    extract_hostile(&archive, &dest);

    assert_ne!(
        fs::read_to_string(dest.join("top/stolen")).ok().as_deref(),
        Some("secret")
    );
}

#[test]
fn symlinks_in_a_real_tag_tarball_do_not_stop_the_extraction_or_the_copy() {
    // The real bundle's 6 symlinks arrive this way. The copy skips them.
    let archive = tag_tarball("1.2.99");
    let temp = crate::test_support::tempdir_outside_framework_roots();
    let dest = temp.path().join("dest");
    fs::create_dir_all(&dest).unwrap();

    extract_tag_tarball(&archive, &dest, &TARBALL_LIMITS).unwrap();

    let bundle = dest.join("amplihack-rs-1.2.99/amplifier-bundle");
    assert!(bundle.join("agents/marker.txt").is_file());
    let copy = temp.path().join("copy");
    copy_dir_recursive(&bundle, &copy).unwrap();
    assert!(fs::symlink_metadata(copy.join("agents/link-to-marker")).is_err());
    assert!(fs::symlink_metadata(copy.join("link-to-agents")).is_err());
}
