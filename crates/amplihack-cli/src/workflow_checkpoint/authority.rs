//! The immutable authority record, private evidence files and scope checks.

use super::actions::{approved, row_paths};
use super::json::{digest, parse, serial};
use super::repo::{
    canonical, env_os, env_text, get_obj, get_str, identity_of, os, path_text, run_cmd, status,
};
use super::{Ctx, Fail, Res, io_refusal, refuse};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A new mode-0700 directory below the assigned `TMPDIR`, outside the worktree.
pub(crate) fn private_dir(ctx: &Ctx) -> Res<PathBuf> {
    let tmp = assigned_tmpdir()?;
    refuse(
        tmp.is_absolute() && tmp.is_dir(),
        "TMPDIR must be an assigned absolute directory",
    )?;
    let tmp = canonical(&tmp);
    refuse(
        !ctx.within_root(&tmp),
        "authority must remain outside the worktree",
    )?;
    let directory = tempfile::Builder::new()
        .prefix("checkpoint-scope-")
        .tempdir_in(&tmp)
        .map_err(io_refusal("private directory"))?
        .keep();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(io_refusal("private directory"))?;
    Ok(directory)
}

/// The assigned `TMPDIR`. Unset is refused; the bytes are kept.
fn assigned_tmpdir() -> Res<PathBuf> {
    env_os("TMPDIR")
        .map(PathBuf::from)
        .ok_or_else(|| Fail::Refusal("TMPDIR must be an assigned absolute directory".into()))
}

/// Write `value` once to a new file and return its digest. Immutability is
/// enforced by the independently held hash and identity, not by a sidecar.
pub(crate) fn retain(path: &Path, value: &Value) -> Res<String> {
    let raw = serial(value);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io_refusal("private record"))?;
    file.write_all(&raw).map_err(io_refusal("private record"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(io_refusal("private record"))?;
    Ok(digest(&raw))
}

pub(crate) fn file_identity(path: &Path) -> Res<String> {
    let info = std::fs::symlink_metadata(path).map_err(io_refusal("private record"))?;
    refuse(
        info.file_type().is_file(),
        "authority is not a regular file",
    )?;
    Ok(identity_of(&info))
}

/// Create a new private file: mode 0600, create-new, no symlink follow.
pub(crate) fn create_private(path: &Path, bytes: &[u8]) -> Res<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(io_refusal("private file"))?;
    file.write_all(bytes).map_err(io_refusal("private file"))
}

/// Observations are evidence only. They never renew scope or authenticate
/// authority and are never consumed as permission by a later invocation.
pub(crate) fn observation(ctx: &Ctx, value: &Value) -> std::io::Result<()> {
    if let Some(directory) = &ctx.observation_directory {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join("operations.jsonl"))?;
        let mut line = serial(value);
        line.push(b'\n');
        file.write_all(&line)?;
    }
    Ok(())
}

pub(crate) fn observe(ctx: &Ctx, value: Value) -> Res<()> {
    observation(ctx, &value).map_err(io_refusal("observation"))
}

impl Ctx {
    pub(crate) fn load_authority(&mut self) -> Res<(Map<String, Value>, Map<String, Value>)> {
        let path = env_os("WF_CHECKPOINT_RECORD")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| Fail::Refusal("authority record identity changed".into()))?;
        refuse(
            Some(file_identity(&path)?) == env_text("WF_CHECKPOINT_RECORD_ID")?,
            "authority record identity changed",
        )?;
        let raw = std::fs::read(&path).map_err(io_refusal("authority record"))?;
        refuse(
            Some(digest(&raw)) == env_text("WF_CHECKPOINT_HASH")?,
            "immutable authority hash mismatch",
        )?;
        let record = match parse(&raw)? {
            Value::Object(map) => map,
            _ => return Err(Fail::Refusal("malformed authority record".into())),
        };
        let binding = get_obj(&record, "binding")?.clone();
        let actual = self.context(get_str(&binding, "phase")?)?;
        for (key, value) in &actual {
            if key != "initial_head" {
                refuse(
                    binding.get(key) == Some(value),
                    &format!("authority binding changed: {key}"),
                )?;
            }
        }
        refuse(
            binding
                .get("invocation")
                .and_then(Value::as_str)
                .map(str::to_owned)
                == env_text("WF_CHECKPOINT_INVOCATION")?,
            "cross-invocation authority",
        )?;
        let private = path.parent().map(Path::to_path_buf).ok_or_else(|| {
            Fail::Refusal("authority is not at its original private location".into())
        })?;
        refuse(
            path.is_absolute()
                && canonical(&path) == path
                && path.file_name().is_some_and(|n| n == "authority.json"),
            "authority is not at its original private location",
        )?;
        refuse(
            private.parent().map(canonical) == Some(canonical(&assigned_tmpdir()?))
                && !self.within_root(&private),
            "authority observations are not in the assigned private directory",
        )?;
        self.observation_directory = Some(private);
        Ok((record, actual))
    }

    /// Pending paths and outgoing history must stay within the approved
    /// paths, and the publication binding and tracking must be unchanged.
    pub(crate) fn scope_check(
        &mut self,
        record: &Map<String, Value>,
        observed_upstream: Option<&str>,
        observed_tracking: Option<(String, String)>,
        publication_view: Option<Map<String, Value>>,
    ) -> Res<()> {
        let allowed = approved(record)?;
        refuse(
            self.pending()?.is_subset(&allowed),
            "unknown pending or staged source path",
        )?;
        let capture = get_obj(record, "capture")?;
        let original = get_obj(capture, "publication")?.clone();
        let publication = match publication_view {
            Some(view) => view,
            None => self.publication(get_obj(record, "binding")?, Some(&original))?,
        };
        self.checked_publication = Some(publication.clone());
        let expected = match observed_upstream {
            Some(u) if !u.is_empty() => Some(u.to_owned()),
            _ => env_text("WF_CHECKPOINT_UPSTREAM")?,
        };
        refuse(
            publication
                .get("upstream")
                .and_then(Value::as_str)
                .map(str::to_owned)
                == expected,
            "upstream changed outside a known fetch operation",
        )?;
        for key in ["ref", "remote", "url_sha256"] {
            refuse(
                publication.get(key) == original.get(key),
                &format!("publication binding changed: {key}"),
            )?;
        }
        let (upstream_ref, base_ref) = match observed_tracking {
            Some((u, b)) => (Some(u), Some(b)),
            None => (
                env_text("WF_CHECKPOINT_UPSTREAM_REF")?,
                env_text("WF_CHECKPOINT_BASE_REF")?,
            ),
        };
        for (key, tracked) in [("upstream_ref", upstream_ref), ("base_ref", base_ref)] {
            refuse(
                publication
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    == tracked,
                &format!("publication tracking changed outside a known operation: {key}"),
            )?;
        }
        for row in self.outgoing(&publication, "HEAD")? {
            refuse(
                row_paths(&row)?.is_subset(&allowed),
                "outgoing history exceeds reviewed scope",
            )?;
        }
        Ok(())
    }

    pub(crate) fn check(&mut self) -> Res<Map<String, Value>> {
        let (record, actual) = self.load_authority()?;
        refuse(
            actual
                .get("initial_head")
                .and_then(Value::as_str)
                .map(str::to_owned)
                == env_text("WF_CHECKPOINT_HEAD")?,
            "unexpected HEAD transition",
        )?;
        self.scope_check(&record, None, None, None)?;
        Ok(record)
    }

    /// Patch identifiers discard whitespace. Replay known changes and compare
    /// complete trees instead, including every source byte and file mode.
    pub(crate) fn verify_replay(
        &self,
        start: &str,
        old_head: &str,
        upstream: &str,
        new_head: &str,
    ) -> Res<()> {
        let original: Vec<String> = self
            .textgit(&["rev-list", "--reverse", &format!("{start}..{old_head}")])?
            .lines()
            .map(str::to_owned)
            .collect();
        let rewritten: Vec<String> = self
            .textgit(&["rev-list", "--reverse", &format!("{upstream}..{new_head}")])?
            .lines()
            .map(str::to_owned)
            .collect();
        refuse(
            original.len() == rewritten.len(),
            "rewritten history changes the known commit count",
        )?;
        let observations = self
            .observation_directory
            .clone()
            .ok_or_else(|| Fail::Refusal("replay requires validated private authority".into()))?;
        let directory = tempfile::Builder::new()
            .prefix("replay-")
            .tempdir_in(&observations)
            .map_err(io_refusal("private replay directory"))?
            .keep();
        let original_objects =
            canonical(
                &self
                    .root
                    .join(self.textgit(&["rev-parse", "--git-path", "objects"])?),
            );
        let objects_bytes = original_objects.as_os_str().as_bytes();
        refuse(
            !objects_bytes.contains(&b'\n') && !objects_bytes.contains(&b'\r'),
            "unsupported object-store path",
        )?;
        let object_format = self.textgit(&["rev-parse", "--show-object-format"])?;
        refuse(
            matches!(object_format.as_str(), "sha1" | "sha256"),
            "unsupported repository object format",
        )?;
        // An object directory alone does not isolate external merge drivers or
        // renormalisation filters. Use private Git metadata, an empty
        // configuration and synthetic home directories. Repository objects are
        // read-only inputs; committed attributes cannot name an executable here.
        for name in ["home", "xdg"] {
            std::fs::create_dir(directory.join(name))
                .map_err(io_refusal("private replay directory"))?;
            std::fs::set_permissions(directory.join(name), std::fs::Permissions::from_mode(0o700))
                .map_err(io_refusal("private replay directory"))?;
        }
        let mut env = super::repo::base_env();
        for (k, v) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_NO_LAZY_FETCH", "1"),
        ] {
            env.push((k.into(), v.into()));
        }
        env.push(("HOME".into(), directory.join("home").into_os_string()));
        env.push((
            "XDG_CONFIG_HOME".into(),
            directory.join("xdg").into_os_string(),
        ));
        let initialized = run_cmd(
            "git",
            &[
                os("init"),
                os("--bare"),
                os("--quiet"),
                os("--template="),
                os("--initial-branch=checkpoint-validation"),
                os(&format!("--object-format={object_format}")),
                directory.as_os_str(),
            ],
            &env,
        )?;
        refuse(
            status(&initialized) == 0,
            "private replay repository preparation failed",
        )?;
        let mut alternates = objects_bytes.to_vec();
        alternates.push(b'\n');
        create_private(&directory.join("objects/info/alternates"), &alternates)?;
        let mut git_dir = OsString::from("--git-dir=");
        git_dir.push(&directory);
        let reference = run_cmd(
            "git",
            &[
                git_dir.as_os_str(),
                os("update-ref"),
                os("refs/heads/checkpoint-validation"),
                os(new_head),
            ],
            &env,
        )?;
        refuse(
            status(&reference) == 0,
            "private replay reference preparation failed",
        )?;
        let mut replayed = Vec::new();
        let result = (|| -> Res<()> {
            let mut parent = upstream.to_owned();
            for (before, after) in original.iter().zip(&rewritten) {
                let old = self.commit_info(before)?;
                let current = self.commit_info(after)?;
                let old_parents = old["parents"]
                    .as_array()
                    .cloned()
                    .ok_or_else(|| Fail::Refusal("malformed commit parents".into()))?;
                refuse(
                    old_parents.len() == 1,
                    "rebase of merge history is unsupported",
                )?;
                refuse(
                    current["parents"] == json!([parent]),
                    "rewritten history changes the expected parent",
                )?;
                let old_parent = old_parents[0]
                    .as_str()
                    .ok_or_else(|| Fail::Refusal("malformed commit parents".into()))?;
                let base = format!("--merge-base={old_parent}");
                let merged = run_cmd(
                    "git",
                    &[
                        os("--literal-pathspecs"),
                        git_dir.as_os_str(),
                        os("merge-tree"),
                        os("--write-tree"),
                        os("--no-messages"),
                        os(&base),
                        os(&parent),
                        os(before),
                    ],
                    &env,
                )?;
                refuse(
                    status(&merged) == 0,
                    &format!(
                        "lossless replay is unsupported or conflicts (exit {})",
                        status(&merged)
                    ),
                )?;
                let tree = super::repo::text(merged.stdout)?.trim().to_owned();
                refuse(
                    tree == self.textgit(&[
                        "rev-parse",
                        "--verify",
                        &format!("{after}^{{tree}}"),
                    ])?,
                    "rewritten source tree differs from the known authorised replay",
                )?;
                replayed.push(
                    json!({"original": before, "rewritten": after, "parent": parent, "tree": tree}),
                );
                parent = after.clone();
            }
            Ok(())
        })();
        let mut private_files = Map::new();
        let mut pending = vec![directory.clone()];
        while let Some(folder) = pending.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    pending.push(path);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    let relative = path
                        .strip_prefix(&directory)
                        .map_err(|_| Fail::Refusal("private replay entry".into()))?;
                    private_files.insert(
                        path_text(relative)?.to_owned(),
                        Value::String(digest(&bytes)),
                    );
                }
            }
        }
        observe(
            self,
            json!({
                "boundary": "lossless-replay-output",
                "directory": path_text(&directory)?,
                "private_object_files": private_files,
                "validated_commits": replayed,
                "writes": "Private Git metadata, refs and replay objects; no user index, worktree, refs or object-store writes.",
            }),
        )?;
        result
    }
}

/// The value of `key` in the authority record's binding.
pub(crate) fn binding_str<'a>(record: &'a Map<String, Value>, key: &str) -> Res<&'a str> {
    get_str(get_obj(record, "binding")?, key)
}

pub(crate) fn capture_publication(record: &Map<String, Value>) -> Res<&Map<String, Value>> {
    get_obj(get_obj(record, "capture")?, "publication")
}
