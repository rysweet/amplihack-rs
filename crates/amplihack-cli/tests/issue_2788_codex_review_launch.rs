//! Issue #2788: only a live, owned review launch directory gets the Codex
//! Git-check bypass. Exercise the real launcher and provisioning commands.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::isolated_amplihack::isolated;
use crate::support::{amplihack_bin::amplihack_bin, exec_stub::write_executable};

const SKIP: &str = "--skip-git-repo-check";
const CONFIG: &str =
    "# fixture configuration must remain unchanged\nsandbox_mode = \"read-only\"\n";
const PROMPT: &[u8] = b"Review this commit and return the Crusty verdict.\n";
const FINAL: &str = "{\"crusty_verdict\":\"CLEAN\",\"concerns\":[]}\n";
/// The #2010 notice for the runner's argv. Only an owned launch names the
/// Git-check bypass, so stderr records whether ownership was established.
const OWNED_NOTICE: &str = "amplihack: codex exec: added --skip-git-repo-check, \
                            --sandbox danger-full-access, \
                            --dangerously-bypass-hook-trust (#2788, #1985)";
const CHECKOUT_NOTICE: &str = "amplihack: codex exec: added --sandbox danger-full-access, \
                               --dangerously-bypass-hook-trust (#1985)";

fn stderr_lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

// Parse values before flags, just as Codex does. Check Git context BEFORE
// producing a final message: without automatic injection the reported bug
// must fail here, rather than accidentally supplying a successful verdict.
const CODEX: &str = r#"#!/bin/sh
case "${1-}" in
  --version) echo 'codex-cli 0.166.0'; exit 0 ;;
  plugin) echo '{"installed":[]}'; exit 0 ;;
esac
printf '%s\000' "$@" > "$HOME/argv"
pwd -P > "$HOME/cwd"
skip=0 final= exec_mode=0 directory= directory_set=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    exec|e) exec_mode=1; shift ;;
    --) shift; break ;;
    --output-last-message|-o) final="$2"; shift 2 ;;
    --cd|-C) [ "$#" -ge 2 ] || exit 2; directory="$2"; directory_set=1; shift 2 ;;
    --cd=*) directory="${1#--cd=}"; directory_set=1; shift ;;
    -C?*) directory="${1#-C}"; directory="${directory#=}"; directory_set=1; shift ;;
    --model|-m|--config|-c|--sandbox|-s|--profile|-p|--add-dir|--enable|--disable)
      [ "$#" -ge 2 ] || exit 2; shift 2 ;;
    --image|-i)
      shift
      while [ "$#" -gt 0 ]; do
        case "$1" in -*) break ;; *) shift ;; esac
      done ;;
    --skip-git-repo-check) skip=$((skip + 1)); shift ;;
    *) shift ;;
  esac
done
printf '%s\n' "$skip" > "$HOME/skip-count"
if [ "$directory_set" = 1 ]; then
  case "$directory" in
    '~') directory="$HOME" ;;
    '~/'*) directory="$HOME/${directory#??}" ;;
  esac
  cd -- "$directory" || exit 2
fi
pwd -P > "$HOME/effective-cwd"
cat > "$HOME/prompt"
if [ "$exec_mode" = 1 ] && ! git rev-parse --is-inside-work-tree >/dev/null 2>&1 && [ "$skip" = 0 ]; then
  echo 'not inside a trusted directory' >&2
  exit 42
fi
if [ -n "$final" ]; then
  printf '{"crusty_verdict":"CLEAN","concerns":[]}\n' > "$final"
fi
printf '{"crusty_verdict":"CLEAN","concerns":[]}\n'
"#;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    tools: PathBuf,
    repo: PathBuf,
    review: PathBuf,
    launch: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let tools = root.join("bin");
        let repo = root.join("repo");
        for dir in [&home, &tools, &repo, &root.join("state")] {
            fs::create_dir(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for dir in [".codex", ".claude", ".amplihack", ".config"] {
            fs::create_dir(home.join(dir)).unwrap();
        }
        fs::write(home.join(".codex/config.toml"), CONFIG).unwrap();
        // Explicit executable discovery; never make a real agent available.
        for tool in [
            "sh", "bash", "cat", "mkdir", "rm", "mv", "cp", "chmod", "git", "uname",
        ] {
            let found = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            assert!(found.status.success(), "missing fixture tool {tool}");
            symlink(
                String::from_utf8(found.stdout).unwrap().trim(),
                tools.join(tool),
            )
            .unwrap();
        }
        write_executable(&tools.join("codex"), CODEX, 0o700);
        write_executable(
            &tools.join("codex-code-mode-host"),
            "#!/bin/sh\nexit 0\n",
            0o700,
        );
        write_executable(&tools.join("node"), "#!/bin/sh\necho v22.14.0\n", 0o700);
        let mut f = Self {
            _temp: temp,
            root,
            home,
            tools,
            repo,
            review: PathBuf::new(),
            launch: PathBuf::new(),
        };
        f.git(&f.repo, &["init", "-q", "-b", "main"]);
        f.git(&f.repo, &["commit", "--allow-empty", "-qm", "baseline"]);
        let out = f
            .command(amplihack_bin())
            .args(["internal", "review-allocate"])
            .arg(&f.repo)
            .arg(f.root.join("state"))
            .output()
            .unwrap();
        f.review = path_output(&out);
        let out = f
            .command(amplihack_bin())
            .args(["internal", "review-launch"])
            .arg(&f.review)
            .output()
            .unwrap();
        f.launch = path_output(&out);
        f
    }

    fn command(&self, binary: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = isolated(binary);
        cmd.current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("CODEX_HOME", self.home.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.home.join(".claude"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("PATH", &self.tools)
            .env("CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        cmd
    }

    fn git(&self, cwd: &Path, args: &[&str]) -> String {
        let out = self
            .command(self.tools.join("git"))
            .current_dir(cwd)
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.templateDir=",
            ])
            .args(args)
            .output()
            .unwrap();
        succeeds(&out);
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn manifest(&self) -> PathBuf {
        self.review.with_file_name("checkout.review-owner.json")
    }

    fn run(&self, cwd: &Path, tail: &[&str]) -> Invocation {
        let final_file = self.root.join("final-message");
        let _ = fs::remove_file(&final_file);
        for name in ["argv", "cwd", "effective-cwd", "skip-count", "prompt"] {
            let _ = fs::remove_file(self.home.join(name));
        }
        let mut child = self
            .command(amplihack_bin())
            .current_dir(cwd)
            .args(["codex", "--"])
            .args(tail)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(PROMPT).unwrap();
        let out = child.wait_with_output().unwrap();
        let argv = fs::read(self.home.join("argv"))
            .unwrap_or_else(|_| panic!("stub never launched: {out:?}"))
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
            .collect();
        assert_eq!(
            fs::read(self.home.join(".codex/config.toml")).unwrap(),
            CONFIG.as_bytes()
        );
        assert_eq!(
            fs::read_to_string(self.home.join("cwd")).unwrap().trim(),
            cwd.canonicalize().unwrap().to_str().unwrap()
        );
        Invocation {
            out,
            argv,
            effective_cwd: PathBuf::from(
                fs::read_to_string(self.home.join("effective-cwd"))
                    .unwrap()
                    .trim(),
            ),
            skip_count: fs::read_to_string(self.home.join("skip-count"))
                .unwrap()
                .trim()
                .parse()
                .unwrap(),
        }
    }

    fn step(&self, cwd: &Path, extra: &[&str]) -> Invocation {
        let final_file = self.root.join("final-message");
        let mut tail = vec![
            "exec",
            "--output-last-message",
            final_file.to_str().unwrap(),
        ];
        tail.extend_from_slice(extra);
        tail.push("-");
        self.run(cwd, &tail)
    }
}

struct Invocation {
    out: Output,
    argv: Vec<String>,
    effective_cwd: PathBuf,
    skip_count: usize,
}

fn succeeds(out: &Output) {
    assert!(
        out.status.success(),
        "status: {}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn path_output(out: &Output) -> PathBuf {
    succeeds(out);
    PathBuf::from(String::from_utf8(out.stdout.clone()).unwrap().trim())
}

#[test]
fn crusty_step_in_provisioned_launch_runs_with_one_skip_flag() {
    let f = Fixture::new();
    let out = f
        .command(f.tools.join("git"))
        .current_dir(&f.launch)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "the review launch directory must remain outside Git"
    );
    let manifest = fs::read(f.manifest()).unwrap();
    let launch = f.step(&f.launch, &[]);
    succeeds(&launch.out);
    assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);
    let stderr = stderr_lines(&launch.out);
    assert!(
        stderr.iter().any(|line| line == OWNED_NOTICE),
        "the notice must name the Git-check bypass: {stderr:?}"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("final-message")).unwrap(),
        FINAL
    );
    assert_eq!(fs::read(f.home.join("prompt")).unwrap(), PROMPT);
    assert_eq!(
        fs::read(f.manifest()).unwrap(),
        manifest,
        "ownership discovery is read-only"
    );
}

#[test]
fn ordinary_linked_and_owned_review_checkouts_keep_codex_git_check() {
    let f = Fixture::new();
    let linked = f.root.join("linked");
    f.git(
        &f.repo,
        &["worktree", "add", "-qb", "pr", linked.to_str().unwrap()],
    );
    assert!(linked.join(".git").is_file());
    for cwd in [&f.repo, &linked, &f.review] {
        let launch = f.step(cwd, &[]);
        succeeds(&launch.out);
        assert_eq!(
            launch.skip_count,
            0,
            "checkout {}: {:?}",
            cwd.display(),
            launch.argv
        );
        let stderr = stderr_lines(&launch.out);
        assert!(
            stderr.iter().any(|line| line == CHECKOUT_NOTICE),
            "checkout {}: {stderr:?}",
            cwd.display()
        );
        assert!(
            !stderr.iter().any(|line| line.contains(SKIP)),
            "checkout {}: {stderr:?}",
            cwd.display()
        );
    }
}

#[test]
fn names_instructions_and_proximity_do_not_establish_launch_ownership() {
    let f = Fixture::new();
    let lookalike = f.root.join("review-fake/review-generation-fake/launch");
    let nested = f.launch.join("nested");
    let sibling = f.launch.with_file_name("other-launch");
    for cwd in [&lookalike, &nested, &sibling] {
        fs::create_dir_all(cwd).unwrap();
        for name in ["AGENTS.md", "CLAUDE.md"] {
            fs::copy(f.launch.join(name), cwd.join(name)).unwrap();
        }
        let launch = f.step(cwd, &[]);
        assert_eq!(launch.skip_count, 0, "{}: {:?}", cwd.display(), launch.argv);
        assert_eq!(launch.out.status.code(), Some(42), "{:?}", launch.out);
        assert!(!f.root.join("final-message").exists());
    }
}

#[test]
fn invalid_or_retired_ownership_evidence_withholds_automatic_skip() {
    let f = Fixture::new();
    let manifest = f.manifest();
    let original = fs::read(&manifest).unwrap();
    for case in [
        "missing",
        "malformed",
        "retired",
        "wrong-checkout",
        "symlink",
        "wrong-schema",
    ] {
        fs::remove_file(&manifest).unwrap();
        match case {
            "missing" => {}
            "malformed" => fs::write(&manifest, b"not json").unwrap(),
            "symlink" => {
                let target = f.root.join("copied-owner.json");
                fs::write(&target, &original).unwrap();
                symlink(target, &manifest).unwrap();
            }
            _ => {
                let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
                match case {
                    "retired" => record["retired"] = true.into(),
                    "wrong-checkout" => record["checkout"] = f.repo.to_str().unwrap().into(),
                    "wrong-schema" => record["schema_version"] = 999.into(),
                    _ => unreachable!(),
                }
                fs::write(&manifest, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        }
        let launch = f.step(&f.launch, &[]);
        assert_eq!(launch.skip_count, 0, "{case}: {:?}", launch.argv);
        assert_eq!(
            launch.out.status.code(),
            Some(42),
            "{case}: {:?}",
            launch.out
        );
        if case != "missing" {
            fs::remove_file(&manifest).unwrap();
        }
        fs::write(&manifest, &original).unwrap();
    }
}

#[test]
fn unrelated_and_special_manifest_candidates_do_not_disrupt_launches() {
    let f = Fixture::new();
    let allocation = f.review.parent().unwrap();
    for name in [
        ".review-owner.json",
        "..review-owner.json",
        "...review-owner.json",
        "broken.review-owner.json",
    ] {
        fs::write(allocation.join(name), b"not a generation").unwrap();
    }
    fs::create_dir(allocation.join("directory.review-owner.json")).unwrap();
    symlink(
        allocation.join("missing"),
        allocation.join("dangling.review-owner.json"),
    )
    .unwrap();
    let fifo = std::ffi::CString::new(
        allocation
            .join("pipe.review-owner.json")
            .as_os_str()
            .as_bytes(),
    )
    .unwrap();
    // SAFETY: fifo is a live, NUL-terminated path for the duration of the call.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);

    let launch = f.step(&f.launch, &[]);
    succeeds(&launch.out);
    assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);

    // With no live manifest, discovery still finishes and leaves Codex's
    // directory check in place, rather than opening the pipe candidate.
    fs::remove_file(f.manifest()).unwrap();
    let launch = f.step(&f.launch, &[]);
    assert_eq!(launch.skip_count, 0, "{:?}", launch.argv);
    assert_eq!(launch.out.status.code(), Some(42), "{:?}", launch.out);
}

#[test]
fn replaced_generation_resources_do_not_retain_eligibility() {
    let f = Fixture::new();
    for resource in [
        &f.launch,
        &f.review,
        &f.launch.with_file_name("repository.git"),
    ] {
        let saved = resource.with_extension("original");
        fs::rename(resource, &saved).unwrap();
        fs::create_dir(resource).unwrap();
        let launch = f.step(&f.launch, &[]);
        assert_eq!(
            launch.skip_count,
            0,
            "replaced {}: {:?}",
            resource.display(),
            launch.argv
        );
        assert_eq!(launch.out.status.code(), Some(42), "{:?}", launch.out);
        fs::remove_dir_all(resource).unwrap();
        fs::rename(saved, resource).unwrap();
    }
}

#[test]
fn launch_path_is_normalized_before_matching_recorded_ownership() {
    let f = Fixture::new();
    let alias = f.root.join("launch-alias");
    symlink(&f.launch, &alias).unwrap();
    for cwd in [&f.launch.join("."), &alias] {
        let launch = f.step(cwd, &[]);
        succeeds(&launch.out);
        assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);
        assert_eq!(
            fs::read_to_string(f.root.join("final-message")).unwrap(),
            FINAL
        );
        assert_eq!(fs::read(f.home.join("prompt")).unwrap(), PROMPT);
    }
}

#[test]
fn explicit_skip_is_preserved_once_in_owned_and_unowned_directories() {
    let f = Fixture::new();
    for cwd in [&f.launch, &f.root] {
        let launch = f.step(cwd, &[SKIP]);
        succeeds(&launch.out);
        assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);
        assert_eq!(
            fs::read_to_string(f.root.join("final-message")).unwrap(),
            FINAL
        );
        assert_eq!(fs::read(f.home.join("prompt")).unwrap(), PROMPT);
    }
}

#[test]
fn flag_like_values_and_prompt_text_do_not_suppress_injection() {
    let f = Fixture::new();
    // The token is a value, not a boolean option. Counting raw argv would
    // mistake these for an explicit caller override.
    let launch = f.step(&f.launch, &["--model", SKIP]);
    succeeds(&launch.out);
    assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);
    let launch = f.run(&f.launch, &["e", "--", SKIP]);
    succeeds(&launch.out);
    assert_eq!(launch.skip_count, 1, "{:?}", launch.argv);
}

#[test]
fn non_exec_invocations_in_owned_launch_do_not_get_skip_flag() {
    let f = Fixture::new();
    for tail in [&["resume", "--last"][..], &["--", "exec"][..]] {
        let launch = f.run(&f.launch, tail);
        succeeds(&launch.out);
        assert_eq!(launch.argv, tail);
        assert_eq!(launch.skip_count, 0);
    }
}

#[test]
fn directory_overrides_cannot_export_owned_launch_authorization() {
    let f = Fixture::new();
    // This directory is outside Git and has no ownership evidence.
    let unowned = f.root.to_str().unwrap();
    let launch = f.launch.to_str().unwrap();
    fs::create_dir(f.launch.join("nested")).unwrap();
    for directory in [unowned, launch, ".", "nested/.."] {
        let expected = Path::new(directory);
        let expected = f.launch.join(expected).canonicalize().unwrap();
        let allowed = expected == f.launch;
        for option in [
            vec!["--cd".to_owned(), directory.to_owned()],
            vec!["-C".to_owned(), directory.to_owned()],
            vec![format!("--cd={directory}")],
            vec![format!("-C{directory}")],
            vec![format!("-C={directory}")],
        ] {
            // Root options, exec options, and options following the prompt
            // positional all select the same effective directory in Codex.
            for position in 0..3 {
                let mut tail = vec!["exec", "-"];
                let insertion = position;
                tail.splice(insertion..insertion, option.iter().map(String::as_str));
                let invocation = f.run(&f.launch, &tail);
                assert_eq!(invocation.effective_cwd, expected, "{tail:?}");
                assert_eq!(invocation.skip_count, usize::from(allowed), "{tail:?}");
                assert_eq!(
                    invocation.out.status.code(),
                    Some(if allowed { 0 } else { 42 }),
                    "{tail:?}: {:?}",
                    invocation.out
                );
            }
        }
    }
    // Normalization accepts a symlink back to the exact owned directory.
    let alias = f.root.join("launch-alias");
    symlink(&f.launch, &alias).unwrap();
    let invocation = f.step(&f.launch, &["--cd", alias.to_str().unwrap()]);
    succeeds(&invocation.out);
    assert_eq!(invocation.skip_count, 1);
    assert_eq!(invocation.effective_cwd, f.launch);

    // Entering an owned directory from an unowned start grants no authority.
    let invocation = f.step(&f.root, &["--cd", launch]);
    assert_eq!(invocation.skip_count, 0);
    assert_eq!(invocation.out.status.code(), Some(42));
    assert!(!f.root.join("final-message").exists());
}

#[test]
fn repeated_and_uncertain_directory_overrides_withhold_automatic_bypass() {
    let f = Fixture::new();
    for options in [
        vec!["--cd", ".", "-C", "."],
        vec!["--cd=.", "--cd=."],
        vec!["--unknown-option=opaque", "-C", "."],
    ] {
        let invocation = f.step(&f.launch, &options);
        assert_eq!(
            invocation.skip_count, 0,
            "{options:?}: {:?}",
            invocation.argv
        );
        assert_eq!(invocation.out.status.code(), Some(42), "{options:?}");
        assert!(!f.root.join("final-message").exists());
    }
    // A caller's explicit bypass survives even an ambiguous or escaping cd.
    for options in [
        vec!["--cd", ".", "-C", ".", SKIP],
        vec!["--cd", f.root.to_str().unwrap(), SKIP],
    ] {
        let invocation = f.step(&f.launch, &options);
        succeeds(&invocation.out);
        assert_eq!(invocation.skip_count, 1, "{:?}", invocation.argv);
        assert_eq!(
            fs::read_to_string(f.root.join("final-message")).unwrap(),
            FINAL
        );
    }
}

#[test]
fn directory_option_text_in_values_and_prompts_is_not_an_override() {
    let f = Fixture::new();
    for tail in [
        vec!["exec", "--model", "--cd", "-"],
        vec!["exec", "--image=--cd=unowned", "-"],
        vec!["e", "--", "--cd"],
        vec!["exec", "--", "-C/unowned"],
        vec!["exec", "Explain --cd and -C without changing directories"],
    ] {
        let invocation = f.run(&f.launch, &tail);
        succeeds(&invocation.out);
        assert_eq!(invocation.skip_count, 1, "{tail:?}: {:?}", invocation.argv);
        assert_eq!(invocation.effective_cwd, f.launch);
    }
}

#[test]
fn home_expansion_and_symlink_parent_components_cannot_export_authorization() {
    let f = Fixture::new();
    let outer = f.root.join("outer");
    let unowned = f.root.join("unowned");
    fs::create_dir_all(outer.join("child")).unwrap();
    fs::create_dir(&unowned).unwrap();
    symlink(outer.join("child"), f.launch.join("a")).unwrap();
    symlink(&unowned, f.launch.join("b")).unwrap();
    symlink(&f.launch, outer.join("b")).unwrap();
    // Following links before resolving '..' appears to select the launch,
    // but Codex's logical normalization selects the unowned 'b' instead.
    assert_eq!(f.launch.join("a/../b").canonicalize().unwrap(), f.launch);
    let invocation = f.step(&f.launch, &["--cd", "a/../b"]);
    assert_eq!(invocation.skip_count, 0);
    assert_eq!(invocation.effective_cwd, unowned);
    assert_eq!(invocation.out.status.code(), Some(42));
    assert!(!f.root.join("final-message").exists());

    // Codex expands a leading '~'; a literal symlink is not proof that the
    // selected home directory is this launch. Explicit bypass is untouched.
    symlink(&f.launch, f.launch.join("~")).unwrap();
    let invocation = f.step(&f.launch, &["--cd", "~"]);
    assert_eq!(invocation.skip_count, 0);
    assert_eq!(invocation.effective_cwd, f.home);
    assert_eq!(invocation.out.status.code(), Some(42));
    let invocation = f.step(&f.launch, &["--cd", "~", SKIP]);
    succeeds(&invocation.out);
    assert_eq!(invocation.skip_count, 1);
}
