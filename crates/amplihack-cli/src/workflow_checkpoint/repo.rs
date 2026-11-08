//! Repository I/O: the Git wrapper, step-04 ownership, pending paths, file
//! identities and the advertisement transports.

use super::advert::AdvertMap;
use super::json::{b64, digest, literal, parse};
use super::{Ctx, Fail, Res, Selectors, io_refusal, refuse};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub(crate) type Env = Vec<(OsString, OsString)>;

pub(crate) fn status(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

/// Which Git builder a spawn uses. It comes only from the action row's
/// `Selectors` column (`From<Selectors>`), is stored once by
/// [`Ctx::for_row`], and is never chosen by a handler. There is no default:
/// every construction names the row or the builder.
///
/// `Strip` re-runs the selector refusal before every spawn (see
/// [`git_command`]), so a bounded-route spawn refuses loudly when a selector
/// is set instead of silently writing the caller's repository.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GitEnv {
    /// `amplihack_git::selector_inheriting_command()`: 918c ran git with
    /// `os.environ` unchanged, so the caller's selectors choose the
    /// repository and index.
    Inherit,
    /// `amplihack_git::command()`: selectors removed after `selectors()` has
    /// refused any that were set (the 17.116 bounded route).
    Strip,
}

impl From<Selectors> for GitEnv {
    fn from(selectors: Selectors) -> Self {
        match selectors {
            Selectors::Ignored => GitEnv::Inherit,
            Selectors::Context => GitEnv::Strip,
        }
    }
}

/// The only place inside `workflow_checkpoint` that builds a Git child.
///
/// `Strip` checks `FORBIDDEN_SELECTORS` again before returning a command. On
/// the bounded route `selectors()` has already refused by this point, so the
/// check never changes a 918c outcome; it guarantees that no Git child starts
/// on that route while a selector is set, whatever order a handler runs in.
pub(crate) fn git_command(git_env: GitEnv) -> Res<Command> {
    match git_env {
        GitEnv::Inherit => Ok(amplihack_git::selector_inheriting_command()),
        GitEnv::Strip => {
            selectors()?;
            Ok(amplihack_git::command())
        }
    }
}

/// A command for `program`. Git goes through [`git_command`]; any other
/// program is started as given. A path to git (`/usr/bin/git`) is refused
/// rather than started raw, so the builder choice cannot be bypassed.
pub(crate) fn command_for(program: &str, git_env: GitEnv) -> Res<Command> {
    if program == "git" {
        return git_command(git_env);
    }
    if Path::new(program).file_name().is_some_and(|n| n == "git") {
        return Err(Fail::Crash(format!(
            "git must be run by name, not {program}"
        )));
    }
    Ok(Command::new(program))
}

/// Run a command with the inherited environment plus `env`, no stdin. A
/// command that cannot start is a refusal. `git_env` has no default: every
/// caller states which builder its row uses.
pub(crate) fn run_cmd(program: &str, args: &[&OsStr], env: &Env, git_env: GitEnv) -> Res<Output> {
    command_for(program, git_env)?
        .args(args)
        .stdin(Stdio::null())
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output()
        .map_err(|e| Fail::Refusal(format!("could not run {program}: {e}")))
}

/// Run an advertisement transport: [`run_cmd`] for git with the builder
/// from [`Ctx::bounded_git_env`]. A transport that cannot start is the
/// caught `OSError` refusal 918c reported.
///
/// Audit (918c256cf): the only callers are `publication_transport` and
/// `destination_tip`, reached from `before`, `sync-fetch-mode`,
/// `destination-is-head` and `verify-pushed-ref`, all `Selectors::Context`.
/// `advertised-ref` (ORIGIN_REF_PY) reads a saved advertisement and spawns
/// nothing; its `git ls-remote` runs in `workflow_pr_head_sync.sh`, whose
/// own environment already inherits. The `GitEnv` is a required argument,
/// so every caller names its builder; callers pass
/// [`Ctx::bounded_git_env`], and an inheriting builder is refused here as
/// well, so a transport never starts with the caller's selectors.
pub(crate) fn run_transport(git_env: GitEnv, args: &[&OsStr], env: &Env) -> Res<Output> {
    if git_env == GitEnv::Inherit {
        return Err(Fail::Crash(
            "advertisement transport given the selector-inheriting GitEnv".into(),
        ));
    }
    run_cmd("git", args, env, git_env)
}

pub(crate) fn os(s: &str) -> &OsStr {
    OsStr::new(s)
}

pub(crate) fn base_env() -> Env {
    vec![
        ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
        ("GIT_NO_REPLACE_OBJECTS".into(), "1".into()),
    ]
}

pub(crate) fn strip_newlines(mut raw: Vec<u8>) -> Vec<u8> {
    while raw.last() == Some(&b'\n') {
        raw.pop();
    }
    raw
}

pub(crate) fn split_nul(raw: &[u8]) -> BTreeSet<Vec<u8>> {
    raw.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

pub(crate) use super::advert::lines;

/// Git output that the caller compares as text. Non-UTF-8 output is refused
/// rather than replaced, so two different byte strings never compare equal.
pub(crate) fn text(raw: Vec<u8>) -> Res<String> {
    String::from_utf8(strip_newlines(raw)).map_err(|_| Fail::Refusal("non-UTF-8 Git output".into()))
}

/// A private path as JSON text. Non-UTF-8 paths are refused.
pub(crate) fn path_text(path: &Path) -> Res<&str> {
    path.to_str()
        .ok_or_else(|| Fail::Refusal("non-UTF-8 private path".into()))
}

impl Ctx {
    /// The builder for helpers that only the bounded (Context) route
    /// reaches. Fails closed if one of them is ever reached from an Ignored
    /// row, instead of silently stripping or inheriting.
    pub(crate) fn bounded_git_env(&self) -> Res<GitEnv> {
        match self.git_env {
            GitEnv::Strip => Ok(GitEnv::Strip),
            GitEnv::Inherit => Err(Fail::Crash(
                "bounded-route Git helper reached from a selector-inheriting action".into(),
            )),
        }
    }

    /// `git --literal-pathspecs -C <root> ARGS`. Exit 79 always leaves with
    /// 79. Other failures refuse, unless `optional`, which yields no output.
    /// Git errors can repeat a credential-bearing URL, so only the status is
    /// reported.
    pub(crate) fn git_with(&self, args: &[&OsStr], optional: bool) -> Res<Vec<u8>> {
        let mut env = base_env();
        env.push(("GIT_NO_LAZY_FETCH".into(), "1".into()));
        let mut full: Vec<&OsStr> =
            vec![os("--literal-pathspecs"), os("-C"), self.root.as_os_str()];
        full.extend_from_slice(args);
        let output = run_cmd("git", &full, &env, self.bounded_git_env()?)?;
        match status(&output) {
            0 => Ok(output.stdout),
            79 => Err(Fail::Terminal79),
            _ if optional => Ok(Vec::new()),
            code => Err(Fail::Refusal(format!(
                "Git inspection failed (exit {code})"
            ))),
        }
    }

    pub(crate) fn git(&self, args: &[&str]) -> Res<Vec<u8>> {
        let args: Vec<&OsStr> = args.iter().map(|a| os(a)).collect();
        self.git_with(&args, false)
    }

    pub(crate) fn git_opt(&self, args: &[&str]) -> Res<Vec<u8>> {
        let args: Vec<&OsStr> = args.iter().map(|a| os(a)).collect();
        self.git_with(&args, true)
    }

    pub(crate) fn textgit(&self, args: &[&str]) -> Res<String> {
        text(self.git(args)?)
    }

    pub(crate) fn textgit_opt(&self, args: &[&str]) -> Res<String> {
        text(self.git_opt(args)?)
    }

    pub(crate) fn within_root(&self, path: &Path) -> bool {
        path.starts_with(&self.root)
    }
}

/// `os.path.realpath`: resolve symlinks; parts that do not exist are kept.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(directory) => directory.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    if let Ok(resolved) = std::fs::canonicalize(&absolute) {
        return resolved;
    }
    let mut normal = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    match (normal.parent(), normal.file_name()) {
        (Some(parent), Some(name)) if parent != normal => canonical(parent).join(name),
        _ => normal,
    }
}

pub(crate) fn lexists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

pub(crate) fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

pub(crate) fn identity_of(meta: &std::fs::Metadata) -> String {
    format!("{}:{}", meta.dev(), meta.ino())
}

pub(crate) fn getuid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// An environment value as raw bytes.
pub(crate) fn env_os(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// An environment value that is compared as text. Unset is `None`; a
/// non-UTF-8 value is refused rather than replaced.
pub(crate) fn env_text(name: &str) -> Res<Option<String>> {
    match env_os(name) {
        None => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| Fail::Refusal(format!("{name} is not UTF-8"))),
    }
}

/// An environment value the action cannot run without.
pub(crate) fn env_required(name: &str) -> Res<String> {
    env_text(name)?.ok_or_else(|| Fail::Refusal(format!("missing {name}")))
}

pub(crate) fn get<'a>(map: &'a Map<String, Value>, key: &str) -> Res<&'a Value> {
    map.get(key)
        .ok_or_else(|| Fail::Refusal(format!("missing field '{key}'")))
}

pub(crate) fn get_str<'a>(map: &'a Map<String, Value>, key: &str) -> Res<&'a str> {
    get(map, key)?
        .as_str()
        .ok_or_else(|| Fail::Refusal(format!("field '{key}' must be a string")))
}

pub(crate) fn get_obj<'a>(map: &'a Map<String, Value>, key: &str) -> Res<&'a Map<String, Value>> {
    get(map, key)?
        .as_object()
        .ok_or_else(|| Fail::Refusal(format!("field '{key}' must be an object")))
}

pub(crate) fn get_arr<'a>(map: &'a Map<String, Value>, key: &str) -> Res<&'a Vec<Value>> {
    get(map, key)?
        .as_array()
        .ok_or_else(|| Fail::Refusal(format!("field '{key}' must be a list")))
}

/// These selectors can change the effective repository, object store, index,
/// configuration or literal path interpretation. Refuse them rather than unset
/// them and accidentally inspect a different repository from the operation.
pub(crate) fn selectors() -> Res<()> {
    refuse(
        !super::FORBIDDEN_SELECTORS
            .iter()
            .any(|k| std::env::var_os(k).is_some()),
        "unsupported inherited Git selector",
    )
}

pub(crate) fn worktree_setup() -> Res<Map<String, Value>> {
    let raw = env_required("RECIPE_VAR_worktree_setup")?;
    setup_object(raw.as_bytes())
}

/// Step-04 output as a JSON object.
pub(crate) fn setup_object(raw: &[u8]) -> Res<Map<String, Value>> {
    match parse(raw)? {
        Value::Object(map) => Ok(map),
        _ => Err(Fail::Refusal("missing step-04 worktree ownership".into())),
    }
}

fn protected(default: &str) -> [String; 5] {
    [
        "main".into(),
        "master".into(),
        "develop".into(),
        "trunk".into(),
        default
            .strip_prefix("origin/")
            .unwrap_or(default)
            .to_owned(),
    ]
}

impl Ctx {
    /// Verify the owned linked worktree and return the binding for `phase`.
    pub(crate) fn context(&mut self, phase: &str) -> Res<Map<String, Value>> {
        selectors()?;
        let Some(owner) = env_text("RECIPE_VAR_task_description")?.filter(|o| !o.trim().is_empty())
        else {
            return Err(Fail::Refusal("missing parent owner task".into()));
        };
        if let Some(task) = env_text("TASK_DESCRIPTION")? {
            refuse(task == owner, "inconsistent parent task identity")?;
        }
        let setup = worktree_setup()?;
        let path = setup.get("worktree_path").and_then(Value::as_str);
        let branch = setup.get("branch_name").and_then(Value::as_str);
        let (path, branch) = match (path, branch) {
            (Some(p), Some(b)) if Path::new(p).is_absolute() && !b.is_empty() => (p, b),
            _ => return Err(Fail::Refusal("invalid step-04 ownership".into())),
        };
        // Step 04 may use a local alias while branch_name remains the
        // publication branch (#1762). Both names come from the owner context.
        let local_branch = match setup.get("local_branch") {
            None => branch,
            Some(Value::String(s)) if !s.is_empty() => s.as_str(),
            Some(_) => return Err(Fail::Refusal("invalid step-04 local branch".into())),
        };
        self.root = canonical(Path::new(path));
        refuse(
            Path::new(path) == self.root && !path.contains('\n') && !path.contains('\r'),
            "step-04 path is not canonical and single-line",
        )?;
        let cwd = canonical(&std::env::current_dir().map_err(io_refusal("current directory"))?);
        refuse(
            self.root.is_dir() && cwd == self.root,
            "checkpoint must start at the owned worktree top",
        )?;
        let dot_git = self.root.join(".git");
        refuse(
            dot_git.is_file() && !is_link(&dot_git),
            "not a linked worktree",
        )?;
        refuse(
            Path::new(&self.textgit(&["rev-parse", "--show-toplevel"])?) == self.root,
            "worktree toplevel mismatch",
        )?;
        let gd = canonical(Path::new(
            &self.textgit(&["rev-parse", "--absolute-git-dir"])?,
        ));
        let common = canonical(
            &self
                .root
                .join(self.textgit(&["rev-parse", "--git-common-dir"])?),
        );
        refuse(gd != common, "main worktree is unsupported")?;
        refuse(
            !lexists(&common.join("info").join("grafts")),
            "legacy grafted history is unsupported",
        )?;
        refuse(
            self.git(&["for-each-ref", "--format=%(refname)", "refs/replace/"])?
                .is_empty(),
            "replacement history is unsupported",
        )?;
        refuse(
            self.textgit(&["rev-parse", "--is-shallow-repository"])? == "false",
            "incomplete shallow history is unsupported",
        )?;
        let index = gd.join("index");
        let index_path = self.textgit(&["rev-parse", "--git-path", "index"])?;
        refuse(
            canonical(&self.root.join(index_path)) == index,
            "unsupported index identity",
        )?;
        refuse(
            index.is_file() && !is_link(&index),
            "unsupported index file",
        )?;
        let raw = std::fs::read(&index).map_err(io_refusal("index"))?;
        refuse(
            raw.len() >= 8
                && &raw[..4] == b"DIRC"
                && matches!(u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]), 2..=4),
            "unsupported index format",
        )?;
        refuse(
            self.textgit(&["rev-parse", "--shared-index-path"])?
                .is_empty(),
            "split index is unsupported",
        )?;
        refuse(
            self.textgit_opt(&["config", "--bool", "core.sparseCheckout"])? != "true",
            "sparse index is unsupported",
        )?;
        refuse(
            matches!(
                self.textgit_opt(&["config", "--get", "core.fsmonitor"])?
                    .as_str(),
                "" | "false"
            ),
            "filesystem-monitor index inspection is unsupported",
        )?;
        let tags = self.git(&["ls-files", "-v", "-z"])?;
        refuse(
            tags.split(|&b| b == 0)
                .all(|row| row.is_empty() || row.starts_with(b"H ")),
            "unsupported index flags",
        )?;
        refuse(
            self.git(&["ls-files", "--unmerged", "-z"])?.is_empty(),
            "unmerged index is unsupported",
        )?;
        let actual = self.textgit(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        refuse(
            actual == local_branch,
            "branch differs from step-04 ownership",
        )?;
        let default = self.textgit_opt(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])?;
        let protected = protected(&default);
        refuse(
            actual != "HEAD"
                && !protected.contains(&actual)
                && branch != "HEAD"
                && !protected.iter().any(|p| p == branch),
            "protected branch",
        )?;
        if local_branch != branch {
            let prefix = format!("{branch}--wt-");
            let suffix = local_branch.strip_prefix(&prefix).unwrap_or("");
            refuse(
                suffix.starts_with(|c: char| ('1'..='9').contains(&c))
                    && suffix.chars().all(|c| c.is_ascii_digit()),
                "invalid step-04 branch alias",
            )?;
            refuse(
                self.textgit_opt(&["config", "--get", &format!("branch.{actual}.remote")])?
                    == "origin"
                    && self.textgit_opt(&["config", "--get", &format!("branch.{actual}.merge")])?
                        == format!("refs/heads/{branch}"),
                "branch alias does not track the step-04 publication branch",
            )?;
        }
        let repo = env_os("RECIPE_VAR_repo_path")
            .ok_or_else(|| Fail::Refusal("missing parent repository".into()))?;
        refuse(
            !repo.is_empty() && Path::new(&repo).is_absolute(),
            "missing parent repository",
        )?;
        let parent = run_cmd(
            "git",
            &[
                os("-C"),
                repo.as_os_str(),
                os("rev-parse"),
                os("--git-common-dir"),
            ],
            &vec![("GIT_OPTIONAL_LOCKS".into(), "0".into())],
            self.bounded_git_env()?,
        )?;
        refuse(
            status(&parent) == 0
                && canonical(
                    &Path::new(&repo)
                        .join(std::ffi::OsString::from_vec(strip_newlines(parent.stdout))),
                ) == common,
            "parent repository mismatch",
        )?;
        for state in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
        ] {
            refuse(!gd.join(state).exists(), "unfinished Git operation")?;
        }
        let initial_head = self.textgit(&["rev-parse", "--verify", "HEAD"])?;
        // Optional free text; absent means empty, as in the Python helper.
        #[allow(clippy::manual_unwrap_or_default)]
        let requirements = match env_text("RECIPE_VAR_final_requirements")? {
            Some(text) => text,
            None => String::new(),
        };
        let binding = json!({
            "owner_task": owner,
            "owner_requirements": requirements,
            "phase": phase,
            "worktree": path_text(&self.root)?,
            "git_dir": path_text(&gd)?,
            "common_dir": path_text(&common)?,
            "index_path": path_text(&index)?,
            "branch": actual,
            "publication_branch": branch,
            "initial_head": initial_head,
        });
        match binding {
            Value::Object(map) => Ok(map),
            _ => Err(Fail::Refusal("malformed binding".into())),
        }
    }

    /// A read-only query with operator filters replaced by a qualified
    /// identity filter. Configuration values never enter output or metadata.
    /// Normal authorised staging and hooks keep their real filters; listing a
    /// path never grants permission to stage it.
    pub(crate) fn inspection(&self, args: &[&str]) -> Res<Vec<u8>> {
        let configured = run_cmd(
            "git",
            &[
                os("--literal-pathspecs"),
                os("-C"),
                self.root.as_os_str(),
                os("config"),
                os("--null"),
                os("--name-only"),
                os("--get-regexp"),
                os(r"^filter\..*\.(clean|process|required)$"),
            ],
            &base_env(),
            self.bounded_git_env()?,
        )?;
        refuse(
            matches!(status(&configured), 0 | 1),
            "filter inspection configuration is unavailable",
        )?;
        let mut prefixes = BTreeSet::new();
        for key in split_nul(&configured.stdout) {
            let dot = key.iter().rposition(|&b| b == b'.');
            let ok = dot.is_some_and(|d| {
                let (prefix, field) = (&key[..d], &key[d + 1..]);
                prefix.starts_with(b"filter.")
                    && matches!(field, b"clean" | b"process" | b"required")
                    && !prefix.iter().any(|c| matches!(c, b'=' | b'\n' | b'\r'))
            });
            refuse(ok, "unsupported literal filter inspection key")?;
            prefixes.insert(key[..dot.unwrap_or(0)].to_vec());
        }
        let mut overrides: Vec<OsString> = Vec::new();
        if !prefixes.is_empty() {
            let identity = "/bin/cat";
            let tool = std::fs::metadata(identity)
                .map_err(io_refusal("inspection identity executable"))?;
            refuse(
                tool.is_file()
                    && tool.uid() == 0
                    && tool.mode() & 0o111 != 0
                    && tool.mode() & 0o022 == 0,
                "inspection identity executable is not qualified",
            )?;
            for prefix in &prefixes {
                for (field, value) in [("clean", identity), ("process", ""), ("required", "false")]
                {
                    // The key keeps its raw bytes; only the suffix is text.
                    let mut setting = OsString::from_vec(prefix.clone());
                    setting.push(format!(".{field}={value}"));
                    overrides.push("-c".into());
                    overrides.push(setting);
                }
            }
        }
        let mut full: Vec<&OsStr> = overrides.iter().map(|o| o.as_os_str()).collect();
        full.extend(args.iter().map(|a| os(a)));
        self.git_with(&full, false)
    }

    pub(crate) fn clean_index(&self) -> Res<()> {
        refuse(
            self.inspection(&[
                "diff-index",
                "--cached",
                "--raw",
                "--no-ext-diff",
                "--no-renames",
                "-z",
                "HEAD",
            ])?
            .is_empty(),
            "pre-existing staging has no invocation authority",
        )
    }

    pub(crate) fn pending(&self) -> Res<BTreeSet<Vec<u8>>> {
        let tracked = split_nul(&self.inspection(&[
            "diff-files",
            "--name-only",
            "--no-ext-diff",
            "--no-renames",
            "-z",
        ])?);
        let staged = split_nul(&self.inspection(&[
            "diff-index",
            "--cached",
            "--name-only",
            "--no-ext-diff",
            "--no-renames",
            "-z",
            "HEAD",
        ])?);
        let others = split_nul(&self.git(&["ls-files", "--others", "--exclude-standard", "-z"])?);
        // Match the existing untracked-only artefact exclusions. Tracked
        // source always remains visible, even beneath a runtime directory.
        let excluded = |path: &[u8]| {
            let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
            if [&b".amplihack/"[..], b".claude/runtime/", b"target/"]
                .iter()
                .any(|p| path.starts_with(p))
            {
                return true;
            }
            if parts
                .iter()
                .any(|p| *p == b"node_modules" || *p == b"__pycache__" || p.ends_with(b".egg-info"))
            {
                return true;
            }
            for i in 0..parts.len().saturating_sub(1) {
                if parts[i] == b"target" {
                    let mut dir = self.root.clone();
                    for part in &parts[..=i] {
                        dir.push(OsStr::from_bytes(part));
                    }
                    if dir.join("CACHEDIR.TAG").is_file() {
                        return true;
                    }
                }
            }
            false
        };
        let mut result: BTreeSet<Vec<u8>> = tracked.union(&staged).cloned().collect();
        result.extend(others.into_iter().filter(|p| !excluded(p)));
        Ok(result)
    }

    pub(crate) fn identity(&self, path: &[u8]) -> Res<Value> {
        literal(path)?;
        let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
        let mut parent = self.root.clone();
        for part in &parts[..parts.len() - 1] {
            parent.push(OsStr::from_bytes(part));
            refuse(!is_link(&parent), "symlink parent is unsupported")?;
        }
        let full = self.root.join(OsStr::from_bytes(path));
        let Ok(info) = std::fs::symlink_metadata(&full) else {
            return Ok(
                json!({"path_b64": b64(path), "mode": "000000", "type": "deletion", "sha256": null}),
            );
        };
        let (payload, mode, kind) = if info.file_type().is_symlink() {
            (
                std::fs::read_link(&full)
                    .map_err(io_refusal("candidate link"))?
                    .into_os_string()
                    .into_vec(),
                "120000",
                "symlink",
            )
        } else {
            refuse(
                info.file_type().is_file(),
                "candidate is a directory, submodule or special file",
            )?;
            let mode = if info.mode() & 0o111 != 0 {
                "100755"
            } else {
                "100644"
            };
            (
                std::fs::read(&full).map_err(io_refusal("candidate file"))?,
                mode,
                "file",
            )
        };
        Ok(json!({"path_b64": b64(path), "mode": mode, "type": kind, "sha256": digest(&payload)}))
    }

    pub(crate) fn commits(&self) -> Res<BTreeSet<String>> {
        let raw = self.git(&[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ])?;
        let mut commits = BTreeSet::new();
        for row in lines(&raw) {
            let mut fields = row.split(|&b| b == b' ').filter(|f| !f.is_empty());
            if let (Some(oid), Some(b"commit")) = (fields.next(), fields.next()) {
                commits.insert(text(oid.to_vec())?);
            }
        }
        Ok(commits)
    }

    pub(crate) fn commit_info(&self, oid: &str) -> Res<Value> {
        let row = self.textgit(&["rev-list", "--parents", "-n", "1", oid])?;
        let row: Vec<&str> = row.split_whitespace().collect();
        refuse(row.first() == Some(&oid), "missing commit identity")?;
        let parents: Vec<String> = row[1..].iter().map(|s| (*s).to_owned()).collect();
        let mut paths = BTreeSet::new();
        if parents.is_empty() {
            paths.extend(split_nul(&self.git(&[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "--no-ext-diff",
                "--no-renames",
                "-r",
                "-z",
                oid,
            ])?));
        } else {
            for parent in &parents {
                paths.extend(split_nul(&self.git(&[
                    "diff-tree",
                    "--no-commit-id",
                    "--name-only",
                    "--no-ext-diff",
                    "--no-renames",
                    "-r",
                    "-z",
                    parent,
                    oid,
                ])?));
            }
        }
        let paths: Vec<String> = paths.iter().map(|p| b64(p)).collect();
        Ok(json!({"commit": oid, "parents": parents, "paths": paths}))
    }
}

impl Ctx {
    /// A command-local alias inspects the exact push destination, even when
    /// origin fetches elsewhere. Its URL is neither an argv value nor retained
    /// in a public capture, and its bytes are passed unchanged. No repository
    /// or global configuration changes.
    pub(crate) fn publication_transport(
        &self,
        publication: &Map<String, Value>,
    ) -> Res<(Env, String)> {
        let alias = format!("workflow-checkpoint-{}", uuid::Uuid::new_v4().simple());
        let push_url =
            strip_newlines(self.git(&["remote", "get-url", "--push", "--all", "origin"])?);
        let mut env = base_env();
        env.push(("GIT_CONFIG_COUNT".into(), "1".into()));
        env.push((
            "GIT_CONFIG_KEY_0".into(),
            format!("remote.{alias}.url").into(),
        ));
        env.push(("GIT_CONFIG_VALUE_0".into(), OsString::from_vec(push_url)));
        let resolved = run_transport(
            self.bounded_git_env()?,
            &[
                os("--literal-pathspecs"),
                os("-C"),
                self.root.as_os_str(),
                os("ls-remote"),
                os("--get-url"),
                os("--"),
                os(&alias),
            ],
            &env,
        )?;
        if status(&resolved) == 79 {
            return Err(Fail::Terminal79);
        }
        refuse(
            status(&resolved) == 0
                && digest(&strip_newlines(resolved.stdout)) == get_str(publication, "url_sha256")?,
            "publication lookup destination differs from the bound push destination",
        )?;
        Ok((env, alias))
    }

    /// Read the destination's advertisement once, validate all of it, and
    /// return the task ref's tip if the destination has it. A terminal 79
    /// leaves with 79; any other failed transport or a malformed
    /// advertisement is a refusal, as 918c `destination_tip` required.
    pub(crate) fn destination_tip(
        &mut self,
        publication: &Map<String, Value>,
    ) -> Res<Option<String>> {
        let (env, alias) = self.publication_transport(publication)?;
        let result = run_transport(
            self.bounded_git_env()?,
            &[
                os("--literal-pathspecs"),
                os("-C"),
                self.root.as_os_str(),
                os("ls-remote"),
                os("--exit-code"),
                os("--"),
                os(&alias),
            ],
            &env,
        )?;
        let code = status(&result);
        if code == 79 {
            eprintln!("publication destination inspection failed (exit 79)");
            return Err(Fail::Terminal79);
        }
        refuse(
            matches!(code, 0 | 2),
            &format!("publication destination inspection failed (exit {code})"),
        )?;
        let advertisement = AdvertMap::parse(&result.stdout, code).map_err(Fail::Advert)?;
        let tip = advertisement
            .tip(get_str(publication, "ref")?.as_bytes())
            .map(str::to_owned);
        // This process-local view is never permission, a receipt or a field.
        self.advertisements.insert(
            get_str(publication, "url_sha256")?.to_owned(),
            advertisement,
        );
        Ok(tip)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_python_splitlines() {
        assert_eq!(
            lines(b"a\nb\r\nc\rd"),
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]
        );
        assert_eq!(lines(b"a\n"), vec![b"a".to_vec()]);
        assert!(lines(b"").is_empty());
    }

    #[test]
    fn canonical_keeps_missing_parts() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            canonical(&dir.path().join("missing/x")),
            real.join("missing/x")
        );
        assert_eq!(canonical(&dir.path().join("a/../b")), real.join("b"));
    }
}
