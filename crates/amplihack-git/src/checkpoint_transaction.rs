//! Private-index checkpoint transactions preserve the caller's staged entries.
use crate::publication_validation::{PolicyRefusal, WorkflowResult};
use crate::{
    task_scope::TaskScope,
    workspace_validation::{Workspace, bytes, run},
};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

struct IndexLock(Option<PathBuf>);
impl Drop for IndexLock {
    fn drop(&mut self) {
        if let Some(path) = &self.0
            && let Err(e) = fs::remove_file(path)
        {
            eprintln!("cannot remove checkpoint index lock: {e}");
        }
    }
}
fn names(raw: Vec<u8>) -> Result<BTreeSet<String>> {
    raw.split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8(b.to_vec()).context("unsupported non-UTF-8 source filename"))
        .collect()
}
fn changed(root: &Path) -> Result<BTreeSet<String>> {
    names(bytes(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--cached",
            "--ita-visible-in-index",
            "--no-renames",
            "--name-only",
            "-z",
            "HEAD",
        ],
    )?)
}
fn git_index(root: &Path, index: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::command_in(root)
        .args(["-c", "core.fsmonitor=false", "--no-optional-locks"])
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()?;
    if !out.status.success() {
        bail!(
            "checkpoint Git command failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out.stdout)
}

/// Hook entry point: only explicitly selected private indexes can be inspected.
pub fn validate_index(root: &Path, index: &Path, allowed: &Path) -> Result<()> {
    let workspace = Workspace::open(root)?;
    let index = ValidatedIndex::open(root, index)?.path().to_owned();
    if !fs::symlink_metadata(allowed)?.is_file() {
        bail!("private authority must be a regular file");
    }
    let allowed = allowed.canonicalize()?;
    if !index.starts_with(workspace.git_dir())
        || index.parent() != allowed.parent()
        || !index
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n.to_string_lossy().starts_with("checkpoint-"))
    {
        bail!("unvalidated checkpoint index location");
    }
    let scope = TaskScope::read_private(&workspace, &fs::read(allowed)?)?;
    let indexed = names(git_index(root, &index, &["ls-files", "-z"])?)?;
    for path in indexed {
        if amplihack_types::paths::owned_runtime::prohibited(&path) {
            bail!("prohibited runtime path in complete checkpoint index: {path}");
        }
    }
    let paths = names(git_index(
        root,
        &index,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--cached",
            "--no-renames",
            "--name-only",
            "-z",
            "HEAD",
        ],
    )?)?;
    for path in paths {
        scope.require(&path)?;
    }
    if !git_index(root, &index, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        bail!("hook left an unmerged index");
    }
    Ok(())
}

pub fn checkpoint(root: &Path, message: &str, stage_only: bool) -> Result<()> {
    checkpoint_into(root, message, stage_only, &mut WorkflowResult::default())
}

pub fn checkpoint_into(
    root: &Path,
    message: &str,
    stage_only: bool,
    result: &mut WorkflowResult,
) -> Result<()> {
    let workspace = Workspace::open(root)?;
    let scope = TaskScope::from_recipe(&workspace)?;
    scope.check_workspace(&workspace)?;
    let index = workspace.git_dir().join("index");
    let lock_path = workspace.git_dir().join("index.lock");
    let mut lock = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
        .context("checkpoint cannot lock Git index; staged work left untouched")?;
    let mut index_lock = IndexLock(Some(lock_path.clone()));
    if !fs::symlink_metadata(&index)?.is_file() {
        bail!("checkpoint requires a regular Git index; original index preserved");
    }
    if !bytes(root, &["ls-files", "--unmerged", "-z"])?.is_empty() {
        bail!("checkpoint refuses an unmerged index");
    }
    let original_head = run(root, &["rev-parse", "--verify", "HEAD^{commit}"])?
        .trim()
        .to_owned();
    let head_ref = run(root, &["symbolic-ref", "--quiet", "HEAD"])?
        .trim()
        .to_owned();
    let original_index = fs::read(&index).context("cannot snapshot original index")?;
    lock.set_permissions(fs::metadata(&index)?.permissions())?;
    if original_index.get(..4) != Some(b"DIRC")
        || !matches!(original_index.get(4..8), Some([0, 0, 0, 2] | [0, 0, 0, 3]))
    {
        bail!("unsupported Git index format; staged work left untouched");
    }
    let protected = changed(root)?;
    for path in &protected {
        if amplihack_types::paths::owned_runtime::prohibited(path) {
            bail!("prohibited pre-existing staged entry: {path}; index preserved");
        }
    }
    let mut candidates = names(bytes(
        root,
        &["diff", "--no-renames", "--name-only", "-z", "HEAD"],
    )?)?;
    candidates.extend(names(bytes(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?)?);
    candidates.retain(|p| scope.permits(p));
    // Candidate inventories can restrict the selection but never grant authority.
    if let Some(raw) = std::env::var_os("RECIPE_VAR_ops_modified_files") {
        let raw = raw
            .into_string()
            .map_err(|_| PolicyRefusal("unsupported candidate encoding".to_owned()))?;
        let selected: Vec<String> =
            serde_json::from_str(&raw).context("invalid candidate filename array")?;
        for path in &selected {
            if !crate::task_scope::valid_path(path) {
                bail!("invalid candidate filename: {path:?}");
            }
            scope.require(path)?;
        }
        candidates.retain(|path| selected.contains(path));
    }
    for candidate in &candidates {
        if protected.iter().any(|p| {
            p == candidate
                || p.starts_with(&format!("{candidate}/"))
                || candidate.starts_with(&format!("{p}/"))
        }) {
            bail!(
                "candidate conflicts with pre-existing staged entry: {candidate}; index preserved"
            );
        }
    }
    if candidates.is_empty() {
        result.reconciled = true;
        return Ok(());
    }
    let scratch = tempfile::Builder::new()
        .prefix("checkpoint-")
        .tempdir_in(workspace.git_dir())?;
    let private = scratch.path().join("index");
    let metadata = entry_metadata(root, &index, &candidates)?;
    if stage_only {
        fs::write(&private, &original_index)?;
    } else {
        git_index(root, &private, &["read-tree", "HEAD"])?;
    }
    let mut args = vec!["--literal-pathspecs", "add", "--"];
    args.extend(candidates.iter().map(String::as_str));
    git_index(root, &private, &args)?;
    if stage_only {
        for path in names(git_index(root, &private, &["ls-files", "-z"])?)? {
            if amplihack_types::paths::owned_runtime::prohibited(&path) {
                bail!("prohibited runtime path in index: {path}");
            }
        }
        preserve_metadata(root, &private, &candidates, &metadata)?;
        if fs::read(&index)? != original_index {
            bail!("real index changed during staging; refusing replacement");
        }
        lock.write_all(&fs::read(&private)?)?;
        lock.sync_all()?;
        fs::rename(&lock_path, &index)?;
        index_lock.0 = None;
        result.reconciled = true;
        return Ok(());
    }
    let selected = scope.select(candidates.clone())?;
    let authority = scratch.path().join("scope.json");
    fs::write(&authority, serde_json::to_vec(&selected)?)?;
    validate_index(root, &private, &authority)?;
    // Prove preservation before running a commit, using the exact proposed tree.
    let tree = String::from_utf8(git_index(root, &private, &["write-tree"])?)?
        .trim()
        .to_owned();
    let reconciled = scratch.path().join("reconciled");
    fs::write(&reconciled, &original_index)?;
    reconcile(root, &reconciled, &tree, &candidates)?;
    preserve_metadata(root, &reconciled, &candidates, &metadata)?;
    // Wrap all normal hooks; each runs with the private index. Validation after
    // every index-mutating commit hook closes the final commit-msg boundary.
    let hooks = scratch.path().join("hooks");
    fs::create_dir(&hooks)?;
    let original_hooks = PathBuf::from(
        run(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", "hooks"],
        )?
        .trim(),
    );
    match fs::read_dir(&original_hooks) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.context("cannot inspect normal Git hooks")?;
                let destination = hooks.join(entry.file_name());
                #[cfg(unix)]
                std::os::unix::fs::symlink(entry.path(), &destination)?;
                #[cfg(not(unix))]
                {
                    if entry.file_type()?.is_dir() {
                        bail!("hook subdirectories are unsupported on this platform");
                    }
                    fs::copy(entry.path(), &destination)?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot preserve normal Git hooks"),
    }
    let executable = std::env::current_exe()?;
    let commit_record = scratch.path().join("committed-oid");
    for name in [
        "pre-commit",
        "prepare-commit-msg",
        "commit-msg",
        "post-commit",
        "reference-transaction",
    ] {
        let original = PathBuf::from(
            run(
                root,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    &format!("hooks/{name}"),
                ],
            )?
            .trim(),
        );
        let wrapper = hooks.join(name);
        // Replace this entry rather than writing through a copied hook symlink.
        match fs::remove_file(&wrapper) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("cannot install checkpoint hook wrapper"),
        }
        let mut script = format!(
            "#!/bin/sh\nif [ -x {} ]; then {} \"$@\" || exit $?; fi\n",
            quote(&original),
            quote(&original)
        );
        if ["pre-commit", "prepare-commit-msg", "commit-msg"].contains(&name) {
            script.push_str(&format!(
                "{} hygiene workflow validate-index --repo {} --index {} --authority {}\n",
                quote(&executable),
                quote(root),
                quote(&private),
                quote(&authority)
            ));
        }
        if name == "reference-transaction" {
            let input = scratch.path().join("reference-input.XXXXXX");
            script = format!(
                "#!/bin/sh\ncheckpoint_reference_input=$(mktemp {input}) || exit $?\ntrap 'rm -f -- \"$checkpoint_reference_input\"' 0\ncat > \"$checkpoint_reference_input\" || exit $?\nwhile read -r old new reference; do\n  if [ \"$reference\" = {head_ref} ] || [ \"$reference\" = HEAD ]; then\n    if [ \"$1\" = prepared ]; then\n      [ \"$old\" = {head} ] || {{ echo 'checkpoint HEAD changed concurrently' >&2; exit 1; }}\n    elif [ \"$1\" = committed ]; then\n      printf '%s\\n' \"$new\" > {record} || exit $?\n    fi\n  fi\ndone < \"$checkpoint_reference_input\"\nif [ -x {original} ]; then {original} \"$@\" < \"$checkpoint_reference_input\" || exit $?; fi\n",
                input = quote(&input),
                original = quote(&original),
                head_ref = quote(Path::new(&head_ref)),
                head = quote(Path::new(&original_head)),
                record = quote(&commit_record),
            );
            // Other normal hooks (including post-index-change and the
            // repository's reference hook) can run after commit-msg. Recheck
            // at the prepared reference boundary before HEAD can advance.
            script.push_str(&format!(
                "if [ \"$1\" = prepared ]; then {} hygiene workflow validate-index --repo {} --index {} --authority {} || exit $?; fi\n",
                quote(&executable), quote(root), quote(&private), quote(&authority)
            ));
        }
        fs::write(&wrapper, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700))?;
        }
    }
    if run(root, &["rev-parse", "HEAD"])?.trim() != original_head
        || fs::read(&index)? != original_index
    {
        bail!("HEAD or real index changed before checkpoint; original staged work preserved");
    }
    let out = crate::command_in(root)
        .env("GIT_INDEX_FILE", &private)
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(["commit", "-m", message])
        .output()?;
    eprint!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!(
            "checkpoint commit refused; original index preserved: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let commit = fs::read_to_string(&commit_record).context("commit succeeded but its reference-transaction record is unavailable; original index preserved")?.trim().to_owned();
    result.committed = true;
    result.commit_id = Some(commit.clone());
    let reconciliation = (|| -> Result<()> {
        if run(root, &["rev-parse", &format!("{commit}^")])?.trim() != original_head
            || run(root, &["rev-parse", "HEAD"])?.trim() != commit
        {
            bail!("unexpected checkpoint parent; original index preserved");
        }
        if fs::read(&index)? != original_index {
            bail!("original index changed despite checkpoint lock");
        }
        fs::write(&reconciled, &original_index)?;
        reconcile(root, &reconciled, &commit, &candidates)?;
        preserve_metadata(root, &reconciled, &candidates, &metadata)?;
        if run(root, &["rev-parse", "HEAD"])?.trim() != commit {
            bail!("HEAD changed during reconciliation");
        }
        Ok(())
    })();
    reconciliation.with_context(|| {
        format!("commit {commit} created but index reconciliation failed; do not repeat the commit")
    })?;
    lock.write_all(&fs::read(reconciled)?)?;
    lock.sync_all()?;
    fs::rename(&lock_path, index)?;
    index_lock.0 = None;
    result.reconciled = true;
    Ok(())
}
/// Compare the actual on-disk entry bytes, including flags and stat fields.
/// Reject extensions whose per-entry state cannot be proved unchanged. TREE
/// is derived cache data, not protected entry metadata.
fn entry_metadata(
    root: &Path,
    index: &Path,
    candidates: &BTreeSet<String>,
) -> Result<BTreeMap<String, Vec<u8>>> {
    let raw = fs::read(index)?;
    let version = raw.get(4..8).context("truncated Git index header")?;
    if raw.get(..4) != Some(b"DIRC") || !matches!(version, [0, 0, 0, 2] | [0, 0, 0, 3]) {
        bail!("unsupported Git index format; original index preserved");
    }
    let hash_size = match run(root, &["rev-parse", "--show-object-format"])?.trim() {
        "sha1" => 20,
        "sha256" => 32,
        _ => bail!("unsupported Git object format"),
    };
    let count = u32::from_be_bytes(
        raw.get(8..12)
            .context("truncated Git index count")?
            .try_into()?,
    );
    let end = raw
        .len()
        .checked_sub(hash_size)
        .context("truncated Git index checksum")?;
    let mut position = 12usize;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let start = position;
        let flags_at = start
            .checked_add(40 + hash_size)
            .context("index entry overflow")?;
        let flags = u16::from_be_bytes(
            raw.get(flags_at..flags_at + 2)
                .context("truncated Git index flags")?
                .try_into()?,
        );
        position = flags_at + 2;
        if flags & 0x4000 != 0 {
            if version != [0, 0, 0, 3] {
                bail!("extended flags require Git index version 3");
            }
            let extended = u16::from_be_bytes(
                raw.get(position..position + 2)
                    .context("truncated extended index flags")?
                    .try_into()?,
            );
            if extended & !0x6000 != 0 {
                bail!("unsupported extended Git entry flags");
            }
            position += 2;
        }
        let name_end = position
            .checked_add(
                raw.get(position..end)
                    .context("invalid Git entry boundary")?
                    .iter()
                    .position(|b| *b == 0)
                    .context("unterminated Git index filename")?,
            )
            .context("index filename overflow")?;
        let name = std::str::from_utf8(&raw[position..name_end])
            .context("unsupported indexed filename")?;
        let entry_length = (name_end - start + 8) & !7;
        position = start
            .checked_add(entry_length)
            .context("index padding overflow")?;
        if position > end || raw[name_end + 1..position].iter().any(|b| *b != 0) {
            bail!("unsupported Git index padding");
        }
        if !candidates.contains(name)
            && entries
                .insert(name.to_owned(), raw[start..position].to_vec())
                .is_some()
        {
            bail!("duplicate Git index filename");
        }
    }
    while position < end {
        let header = raw
            .get(position..position + 8)
            .context("truncated Git index extension")?;
        let signature = &header[..4];
        let length = u32::from_be_bytes(header[4..].try_into()?) as usize;
        if signature != b"TREE" {
            bail!("unsupported Git index extension {signature:?}; original index preserved");
        }
        position = position
            .checked_add(8)
            .and_then(|p| p.checked_add(length))
            .context("Git index extension overflow")?;
        if position > end {
            bail!("truncated Git index extension data");
        }
    }
    if position != end {
        bail!("invalid Git index entry boundary");
    }
    Ok(entries)
}

fn preserve_metadata(
    root: &Path,
    index: &Path,
    candidates: &BTreeSet<String>,
    original: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    if &entry_metadata(root, index, candidates)? != original {
        bail!("checkpoint cannot preserve protected Git entry metadata; original index preserved");
    }
    Ok(())
}
fn reconcile(root: &Path, index: &Path, tree: &str, candidates: &BTreeSet<String>) -> Result<()> {
    let mut args = vec![
        "--literal-pathspecs",
        "reset",
        "--no-refresh",
        "-q",
        tree,
        "--",
    ];
    args.extend(candidates.iter().map(String::as_str));
    git_index(root, index, &args)?;
    Ok(())
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// An explicitly selected, regular private index belonging to this checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedIndex(PathBuf);
impl ValidatedIndex {
    pub fn open(root: &Path, path: &Path) -> Result<Self> {
        let workspace = Workspace::open(root)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            bail!("private index must be a regular file");
        }
        let canonical = path.canonicalize()?;
        let parent = canonical.parent().context("index has no parent")?;
        if parent.parent() != Some(workspace.git_dir())
            || !parent
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("checkpoint-"))
        {
            bail!("explicit private index does not belong to this checkout");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(parent)?.permissions().mode() & 0o077 != 0 {
                bail!("private index directory is accessible by other users");
            }
        }
        Ok(Self(canonical))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
