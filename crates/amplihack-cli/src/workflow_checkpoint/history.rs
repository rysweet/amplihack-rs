//! Private destination history. Capture copies the destination tip's history
//! into a private bare repository; later checks use it, unchanged, to decide
//! which local commits a push would publish. Nothing is fetched into the task
//! repository.

use super::authority::{file_identity, private_dir, retain};
use super::json::{digest, parse, serial};
use super::repo::{
    Env, base_env, canonical, get_str, getuid, identity_of, lines, os, path_text, run_cmd, status,
    strip_newlines, text,
};
use super::{Ctx, Fail, Res, io_refusal, refuse};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

/// History queries never use operator filters, templates or lazy fetching.
fn history_environment(directory: &Path) -> Env {
    let parent = directory.parent().unwrap_or(directory);
    let mut env = base_env();
    for (k, v) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_ATTR_NOSYSTEM", "1"),
        ("GIT_NO_LAZY_FETCH", "1"),
    ] {
        env.push((k.into(), v.into()));
    }
    env.push(("HOME".into(), parent.join("home").into_os_string()));
    env.push((
        "XDG_CONFIG_HOME".into(),
        parent.join("xdg").into_os_string(),
    ));
    env
}

/// `git --literal-pathspecs --git-dir=<directory> ARGS`; 79 leaves with 79.
/// A transport environment, when given, replaces the private one.
fn history_command(directory: &Path, args: &[&str], env: Option<&Env>) -> Res<Output> {
    let mut git_dir = OsString::from("--git-dir=");
    git_dir.push(directory);
    let mut full: Vec<&OsStr> = vec![os("--literal-pathspecs"), git_dir.as_os_str()];
    full.extend(args.iter().map(|a| os(a)));
    let private = history_environment(directory);
    let output = run_cmd("git", &full, env.unwrap_or(&private))?;
    if status(&output) == 79 {
        return Err(Fail::Terminal79);
    }
    Ok(output)
}

/// Every entry below the private repository, with identity, mode and content
/// digest. Symlinks, special files and foreign or group/other-writable entries
/// are refused.
pub(crate) fn history_files(directory: &Path) -> Res<Map<String, Value>> {
    let mut result = Map::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).map_err(io_refusal("private history"))? {
            let path = entry.map_err(io_refusal("private history"))?.path();
            let info = std::fs::symlink_metadata(&path).map_err(io_refusal("private history"))?;
            let kind = info.file_type();
            refuse(
                info.uid() == getuid()
                    && info.mode() & 0o022 == 0
                    && (kind.is_dir() || kind.is_file()),
                "unsupported private history entry",
            )?;
            let mut row = json!({
                "identity": identity_of(&info),
                "mode": info.mode() & 0o7777,
                "directory": kind.is_dir(),
            });
            if kind.is_file() {
                row["sha256"] = Value::String(digest(
                    &std::fs::read(&path).map_err(io_refusal("private history"))?,
                ));
            } else {
                pending.push(path.clone());
            }
            let relative = path
                .strip_prefix(directory)
                .map_err(|_| Fail::Refusal("unsupported private history entry".into()))?;
            result.insert(path_text(relative)?.to_owned(), row);
        }
    }
    Ok(result)
}

fn object_inventory(directory: &Path) -> Res<Output> {
    history_command(
        directory,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype) %(objectsize)",
        ],
        None,
    )
}

struct Umask(libc::mode_t);

impl Umask {
    fn private() -> Self {
        // SAFETY: umask only swaps the process file-creation mask.
        Umask(unsafe { libc::umask(0o077) })
    }
}

impl Drop for Umask {
    fn drop(&mut self) {
        // SAFETY: restores the mask saved by `private`.
        unsafe { libc::umask(self.0) };
    }
}

impl Ctx {
    /// All preparation writes belong to one private view. A missing remote tip
    /// is never fetched into the implementation repository.
    pub(crate) fn prepare_history(
        &mut self,
        publication: &Map<String, Value>,
        seeds: &[Option<String>],
    ) -> Res<Value> {
        let parent = private_dir(self)?;
        let directory = parent.join("repository");
        for name in ["home", "xdg"] {
            std::fs::create_dir(parent.join(name)).map_err(io_refusal("private history"))?;
            std::fs::set_permissions(parent.join(name), std::fs::Permissions::from_mode(0o700))
                .map_err(io_refusal("private history"))?;
        }
        let object_format = self.textgit(&["rev-parse", "--show-object-format"])?;
        refuse(
            matches!(object_format.as_str(), "sha1" | "sha256"),
            "unsupported history object format",
        )?;
        let mut journal = json!({
            "directory": path_text(&directory)?,
            "worktree": path_text(&self.root)?,
            "object_format": object_format,
            "captured_tip": publication.get("destination_tip").cloned().unwrap_or(Value::Null),
            "destination_sha256": get_str(publication, "url_sha256")?,
            "ref_sha256": digest(get_str(publication, "ref")?.as_bytes()),
            "writes": "Private Git metadata and objects only.",
            "operations": [],
            "complete": false,
        });
        let umask = Umask::private();
        let result = self.prepare_history_steps(
            publication,
            seeds,
            &parent,
            &directory,
            &object_format,
            &mut journal,
        );
        if let Err(Fail::Terminal79) = &result {
            journal["terminal_exit"] = json!(79);
        }
        let written = std::fs::write(parent.join("history-preparation.json"), serial(&journal))
            .and_then(|()| {
                std::fs::set_permissions(
                    parent.join("history-preparation.json"),
                    std::fs::Permissions::from_mode(0o600),
                )
            });
        drop(umask);
        let value = result?;
        written.map_err(io_refusal("private history journal"))?;
        Ok(value)
    }

    fn prepare_history_steps(
        &mut self,
        publication: &Map<String, Value>,
        seeds: &[Option<String>],
        parent: &Path,
        directory: &Path,
        object_format: &str,
        journal: &mut Value,
    ) -> Res<Value> {
        let record = |journal: &mut Value, operation: &str, output: &Output| {
            if let Some(ops) = journal["operations"].as_array_mut() {
                ops.push(json!({"operation": operation, "exit_code": status(output)}));
            }
        };
        let initialized = run_cmd(
            "git",
            &[
                os("init"),
                os("--bare"),
                os("--quiet"),
                os("--template="),
                os("--initial-branch=checkpoint-history"),
                os(&format!("--object-format={object_format}")),
                directory.as_os_str(),
            ],
            &history_environment(directory),
        )?;
        record(journal, "private-init", &initialized);
        if status(&initialized) == 79 {
            return Err(Fail::Terminal79);
        }
        refuse(
            status(&initialized) == 0,
            "private history preparation failed",
        )?;
        let seeds: BTreeSet<&str> = seeds
            .iter()
            .flatten()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .collect();
        let seed_file = parent.join("seed.pack");
        let stream = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&seed_file)
            .map_err(io_refusal("private history seed"))?;
        let mut command = amplihack_git::command();
        command
            .args([
                os("--literal-pathspecs"),
                os("-C"),
                self.root.as_os_str(),
                os("pack-objects"),
                os("--stdout"),
                os("--revs"),
            ])
            .envs(base_env().iter().map(|(k, v)| (k, v)))
            .env("GIT_NO_LAZY_FETCH", "1")
            .stdin(Stdio::piped())
            .stdout(stream)
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(io_refusal("could not run git"))?;
        let input: String = seeds.iter().map(|s| format!("{s}\n")).collect();
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin
                .write_all(input.as_bytes())
                .map_err(io_refusal("private history seed"))?;
        }
        let packed = child
            .wait_with_output()
            .map_err(io_refusal("could not run git"))?;
        record(journal, "read-source-pack-to-private-file", &packed);
        if status(&packed) == 79 {
            return Err(Fail::Terminal79);
        }
        refuse(
            status(&packed) == 0,
            "private history seed preparation failed",
        )?;
        let imported = amplihack_git::command()
            .arg({
                let mut git_dir = OsString::from("--git-dir=");
                git_dir.push(directory);
                git_dir
            })
            .args(["index-pack", "--stdin"])
            .envs(history_environment(directory).iter().map(|(k, v)| (k, v)))
            .stdin(std::fs::File::open(&seed_file).map_err(io_refusal("private history seed"))?)
            .output()
            .map_err(io_refusal("could not run git"))?;
        record(journal, "private-index-pack", &imported);
        if status(&imported) == 79 {
            return Err(Fail::Terminal79);
        }
        refuse(status(&imported) == 0, "private history seed import failed")?;
        std::fs::remove_file(&seed_file).map_err(io_refusal("private history seed"))?;
        let tip = publication
            .get("destination_tip")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty());
        if let Some(tip) = tip {
            let (transport, alias) = self.publication_transport(publication)?;
            let resolved = history_command(
                directory,
                &["ls-remote", "--get-url", "--", &alias],
                Some(&transport),
            )?;
            refuse(
                status(&resolved) == 0
                    && digest(&strip_newlines(resolved.stdout))
                        == get_str(publication, "url_sha256")?,
                "private history destination differs from the captured destination",
            )?;
            let fetched = history_command(
                directory,
                &[
                    "fetch",
                    "--no-write-fetch-head",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--no-auto-maintenance",
                    "--",
                    &alias,
                    tip,
                ],
                Some(&transport),
            )?;
            record(journal, "private-exact-tip-fetch", &fetched);
            refuse(
                status(&fetched) == 0,
                "private destination history preparation failed",
            )?;
        }
        let promisor = std::fs::read_dir(directory.join("objects/pack"))
            .map_err(io_refusal("private history packs"))?
            .flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "promisor"));
        refuse(!promisor, "partial private history is unsupported")?;
        if let Some(tip) = tip {
            let present = history_command(
                directory,
                &["cat-file", "-e", &format!("{tip}^{{commit}}")],
                None,
            )?;
            refuse(
                status(&present) == 0,
                "captured destination is absent from private history",
            )?;
        }
        let checked = history_command(
            directory,
            &["fsck", "--strict", "--full", "--no-reflogs"],
            None,
        )?;
        refuse(
            status(&checked) == 0,
            "private history objects failed validation",
        )?;
        let current = self.destination_tip(publication)?;
        refuse(
            current.as_deref() == publication.get("destination_tip").and_then(Value::as_str),
            "destination changed during history preparation",
        )?;
        refuse(
            !["FETCH_HEAD", "shallow", "objects/info/alternates"]
                .iter()
                .any(|n| directory.join(n).exists()),
            "private history introduced unsupported metadata",
        )?;
        let objects = object_inventory(directory)?;
        refuse(
            status(&objects) == 0,
            "private history inventory is unavailable",
        )?;
        let inventory = json!({
            "files": history_files(directory)?,
            "object_inventory_sha256": digest(&objects.stdout),
        });
        let inventory_path = parent.join("history-inventory.json");
        let inventory_hash = retain(&inventory_path, &inventory)?;
        let info = std::fs::symlink_metadata(directory).map_err(io_refusal("private history"))?;
        let parent_info =
            std::fs::symlink_metadata(parent).map_err(io_refusal("private history"))?;
        let result = json!({
            "directory": path_text(directory)?,
            "identity": identity_of(&info),
            "parent_identity": identity_of(&parent_info),
            "inventory_file": path_text(&inventory_path)?,
            "inventory_identity": file_identity(&inventory_path)?,
            "inventory_sha256": inventory_hash,
            "object_format": object_format,
            "worktree": path_text(&self.root)?,
            "captured_tip": publication.get("destination_tip").cloned().unwrap_or(Value::Null),
            "destination_sha256": get_str(publication, "url_sha256")?,
            "ref_sha256": digest(get_str(publication, "ref")?.as_bytes()),
        });
        journal["complete"] = Value::Bool(true);
        Ok(result)
    }

    pub(crate) fn validate_history(
        &self,
        history: &Map<String, Value>,
        publication: &Map<String, Value>,
    ) -> Res<PathBuf> {
        const KEYS: [&str; 11] = [
            "directory",
            "identity",
            "parent_identity",
            "inventory_file",
            "inventory_identity",
            "inventory_sha256",
            "object_format",
            "worktree",
            "captured_tip",
            "destination_sha256",
            "ref_sha256",
        ];
        refuse(
            history.len() == KEYS.len() && KEYS.iter().all(|k| history.contains_key(*k)),
            "malformed private history descriptor",
        )?;
        let directory = PathBuf::from(get_str(history, "directory")?);
        refuse(
            directory == canonical(&directory) && !self.within_root(&directory),
            "private history location changed",
        )?;
        let parent = directory
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| Fail::Refusal("private history location changed".into()))?;
        let parent_info =
            std::fs::symlink_metadata(&parent).map_err(io_refusal("private history"))?;
        refuse(
            parent_info.is_dir()
                && parent_info.uid() == getuid()
                && parent_info.mode() & 0o7777 == 0o700
                && history.get("parent_identity").and_then(Value::as_str)
                    == Some(identity_of(&parent_info).as_str()),
            "private history parent identity changed",
        )?;
        let info = std::fs::symlink_metadata(&directory).map_err(io_refusal("private history"))?;
        refuse(
            info.is_dir()
                && info.uid() == getuid()
                && info.mode() & 0o022 == 0
                && history.get("identity").and_then(Value::as_str)
                    == Some(identity_of(&info).as_str()),
            "private history identity changed",
        )?;
        refuse(
            history.get("worktree").and_then(Value::as_str) == Some(path_text(&self.root)?)
                && history.get("destination_sha256") == publication.get("url_sha256")
                && history.get("ref_sha256").and_then(Value::as_str)
                    == Some(digest(get_str(publication, "ref")?.as_bytes()).as_str())
                && history.get("object_format").and_then(Value::as_str)
                    == Some(
                        self.textgit(&["rev-parse", "--show-object-format"])?
                            .as_str(),
                    ),
            "private history binding changed",
        )?;
        let path = parent.join("history-inventory.json");
        refuse(
            history.get("inventory_file").and_then(Value::as_str) == Some(path_text(&path)?)
                && history.get("inventory_identity").and_then(Value::as_str)
                    == Some(file_identity(&path)?.as_str()),
            "private history inventory identity changed",
        )?;
        let inventory_info =
            std::fs::symlink_metadata(&path).map_err(io_refusal("private history inventory"))?;
        refuse(
            inventory_info.uid() == getuid() && inventory_info.mode() & 0o7777 == 0o600,
            "private history inventory permissions changed",
        )?;
        let raw = std::fs::read(&path).map_err(io_refusal("private history inventory"))?;
        refuse(
            history.get("inventory_sha256").and_then(Value::as_str) == Some(digest(&raw).as_str()),
            "private history inventory changed",
        )?;
        let saved = parse(&raw)?;
        refuse(
            saved.get("files") == Some(&Value::Object(history_files(&directory)?)),
            "private history files changed",
        )?;
        let actual_format =
            history_command(&directory, &["rev-parse", "--show-object-format"], None)?;
        refuse(
            status(&actual_format) == 0
                && history.get("object_format").and_then(Value::as_str)
                    == Some(text(actual_format.stdout.clone())?.trim()),
            "private history object format changed",
        )?;
        let objects = object_inventory(&directory)?;
        refuse(
            status(&objects) == 0
                && saved.get("object_inventory_sha256").and_then(Value::as_str)
                    == Some(digest(&objects.stdout).as_str()),
            "private history object inventory changed",
        )?;
        if let Some(tip) = history
            .get("captured_tip")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
        {
            let present = history_command(
                &directory,
                &["cat-file", "-e", &format!("{tip}^{{commit}}")],
                None,
            )?;
            refuse(
                status(&present) == 0,
                "captured private history tip is unavailable",
            )?;
        }
        Ok(directory)
    }

    fn history_ancestors(&self, oid: &str, directory: &Path) -> Res<BTreeSet<String>> {
        refuse(
            matches!(oid.len(), 40 | 64)
                && oid
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid explicit history identity",
        )?;
        let available = history_command(
            directory,
            &["cat-file", "-e", &format!("{oid}^{{commit}}")],
            None,
        )?;
        if status(&available) == 0 {
            let result = history_command(directory, &["rev-list", oid], None)?;
            refuse(
                status(&result) == 0,
                "private history ancestry is incomplete",
            )?;
            return Ok(text(result.stdout)?.lines().map(str::to_owned).collect());
        }
        // Only already present implementation objects can describe later
        // normal commits or fetches. This never imports or renews evidence.
        let kind = self.textgit_opt(&["cat-file", "-t", oid])?;
        refuse(
            matches!(kind.as_str(), "commit" | "tag" | "tree" | "blob"),
            "advertised ancestry is unavailable locally",
        )?;
        if matches!(kind.as_str(), "tree" | "blob") {
            return Ok(BTreeSet::new());
        }
        if kind == "tag"
            && self
                .textgit_opt(&["rev-parse", "--verify", &format!("{oid}^{{commit}}")])?
                .is_empty()
        {
            let peeled = self.textgit(&["rev-parse", "--verify", &format!("{oid}^{{}}")])?;
            refuse(
                matches!(
                    self.textgit(&["cat-file", "-t", &peeled])?.as_str(),
                    "tree" | "blob"
                ),
                "unsupported advertised tag target",
            )?;
            return Ok(BTreeSet::new());
        }
        Ok(self
            .textgit(&["rev-list", oid])?
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Commits a push of `head` would add to the destination, oldest first.
    pub(crate) fn outgoing(&self, publication: &Map<String, Value>, head: &str) -> Res<Vec<Value>> {
        let url = get_str(publication, "url_sha256")?;
        let advertised = self
            .advertisements
            .get(url)
            .ok_or_else(|| Fail::Refusal("missing actual remote advertisement".into()))?;
        let tips: BTreeSet<String> =
            match publication.get("destination_tip").and_then(Value::as_str) {
                Some(tip) if !tip.is_empty() => BTreeSet::from([tip.to_owned()]),
                _ => advertised.oids().map(str::to_owned).collect(),
            };
        let history = publication
            .get("history")
            .and_then(Value::as_object)
            .ok_or_else(|| Fail::Refusal("malformed private history descriptor".into()))?;
        let directory = self.validate_history(history, publication)?;
        let mut excluded = BTreeSet::new();
        for tip in &tips {
            excluded.extend(self.history_ancestors(tip, &directory)?);
        }
        let mut rows = Vec::new();
        for oid in lines(&self.git(&["rev-list", "--reverse", head])?) {
            let oid = text(oid)?;
            if !excluded.contains(&oid) {
                rows.push(self.commit_info(&oid)?);
            }
        }
        Ok(rows)
    }
}
