//! Unit tests for the shared command staging swap.

use super::*;
use tempfile::TempDir;

fn verbatim(_path: &Path, body: &str) -> Result<String> {
    Ok(body.to_string())
}

/// Build a request whose scratch root is a sibling of `target`'s parent, i.e.
/// never inside the directory a host tool would scan.
fn request<'a>(
    source: &'a Path,
    target: &'a Path,
    scratch_root: &'a Path,
    retire_root: &'a Path,
) -> StageRequest<'a> {
    StageRequest {
        source,
        target,
        scratch_root,
        retire_root,
        target_is_owned: false,
    }
}

fn stage(request: &StageRequest<'_>) -> Result<StagedCommands> {
    stage_command_files(request, verbatim)
}

struct Fixture {
    _td: TempDir,
    source: PathBuf,
    target: PathBuf,
    scratch: PathBuf,
    amp: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let td = TempDir::new().unwrap();
        let source = td.path().join("source");
        let target = td.path().join("commands").join("amplihack");
        let scratch = td.path().join("scratch");
        let amp = td.path().join(".amplihack");
        fs::create_dir_all(&source).unwrap();
        Self {
            _td: td,
            source,
            target,
            scratch,
            amp,
        }
    }

    fn request(&self) -> StageRequest<'_> {
        request(&self.source, &self.target, &self.scratch, &self.amp)
    }

    fn staging_dir(&self) -> PathBuf {
        scratch_path(&self.scratch, &self.target, ".staging")
    }

    fn backup_dir(&self) -> PathBuf {
        scratch_path(&self.scratch, &self.target, ".old")
    }
}

#[test]
fn prefers_docs_source_over_legacy_claude_source() {
    let td = TempDir::new().unwrap();
    let repo = td.path().join("repo");
    let docs = repo
        .join("docs")
        .join("claude")
        .join("commands")
        .join("amplihack");
    let legacy = repo.join(".claude").join("commands").join("amplihack");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(&legacy).unwrap();

    assert_eq!(command_source_dir(&repo).as_deref(), Some(docs.as_path()));
}

#[test]
fn falls_back_to_legacy_claude_source() {
    let td = TempDir::new().unwrap();
    let repo = td.path().join("repo");
    let legacy = repo.join(".claude").join("commands").join("amplihack");
    fs::create_dir_all(&legacy).unwrap();

    assert_eq!(command_source_dir(&repo).as_deref(), Some(legacy.as_path()));
}

#[test]
fn no_source_directory_yields_none() {
    let td = TempDir::new().unwrap();
    assert!(command_source_dir(&td.path().join("repo")).is_none());
}

/// `find_bundled_framework_root` resolves `repo_root` to `~/.amplihack` on any
/// host with a prior staged install, and the parent probe then evaluates to
/// `$HOME/.claude/commands/amplihack` — the Claude staging target itself.
#[test]
fn parent_probe_that_resolves_to_the_staging_target_is_not_offered_as_a_source() {
    let td = TempDir::new().unwrap();
    let home = td.path();
    let repo = home.join(".amplihack");
    fs::create_dir_all(&repo).unwrap();
    let target = home.join(".claude").join("commands").join("amplihack");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("lock.md"), "staged\n").unwrap();

    assert_eq!(
        command_source_dir(&repo).as_deref(),
        Some(target.as_path()),
        "precondition: the unguarded probe really does resolve to the target"
    );
    assert!(
        command_source_dir_excluding(&repo, Some(&target)).is_none(),
        "staging the target from itself reports a command count that can never refresh"
    );
}

#[test]
fn staging_a_directory_from_itself_is_refused() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.target).unwrap();
    fs::write(fixture.target.join("lock.md"), "v1\n").unwrap();

    let error = stage(&request(
        &fixture.target,
        &fixture.target,
        &fixture.scratch,
        &fixture.amp,
    ))
    .expect_err("source == target must be refused, not reported as success");

    assert!(
        format!("{error:#}").contains("into itself"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("lock.md")).unwrap(),
        "v1\n",
        "the refusal must not disturb the directory"
    );
}

#[test]
fn stages_only_markdown_files() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "# /lock\n").unwrap();
    fs::write(fixture.source.join("notes.txt"), "ignored\n").unwrap();

    let staged = stage(&fixture.request()).unwrap();

    assert_eq!(staged.copied, 1);
    assert!(fixture.target.join("lock.md").is_file());
    assert!(!fixture.target.join("notes.txt").exists());
}

#[test]
fn empty_source_leaves_target_untouched_and_removes_staging() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.target).unwrap();
    fs::write(fixture.target.join("previous.md"), "keep me\n").unwrap();

    let staged = stage(&fixture.request()).unwrap();

    assert_eq!(staged.copied, 0);
    assert!(
        fixture.target.join("previous.md").is_file(),
        "an empty source must not wipe an already-staged command set"
    );
    assert!(
        !fixture.staging_dir().exists(),
        "staging dir must be cleaned"
    );
}

#[test]
fn restaging_an_owned_target_replaces_stale_commands_and_leaves_no_scratch_dirs() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v2\n").unwrap();
    fs::create_dir_all(&fixture.target).unwrap();
    fs::write(fixture.target.join("lock.md"), "v1\n").unwrap();
    fs::write(fixture.target.join("removed.md"), "stale\n").unwrap();

    let mut request = fixture.request();
    request.target_is_owned = true;
    let staged = stage(&request).unwrap();

    assert_eq!(staged.copied, 1);
    assert!(staged.preserved.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.target.join("lock.md")).unwrap(),
        "v2\n"
    );
    assert!(
        !fixture.target.join("removed.md").exists(),
        "a verified-amplihack directory may be replaced wholesale, dropping \
         commands the source no longer ships"
    );
    assert!(!fixture.staging_dir().exists());
    assert!(!fixture.backup_dir().exists());
}

/// The whole point of the ownership check: a directory amplihack cannot prove
/// it owns keeps everything the new command set does not itself provide.
#[test]
fn restaging_an_unowned_target_preserves_files_amplihack_did_not_stage() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v2\n").unwrap();
    fs::create_dir_all(&fixture.target).unwrap();
    fs::write(fixture.target.join("lock.md"), "v1\n").unwrap();
    fs::write(fixture.target.join("my-thing.md"), "mine\n").unwrap();
    fs::create_dir_all(fixture.target.join("my-dir")).unwrap();
    fs::write(fixture.target.join("my-dir").join("note.txt"), "mine\n").unwrap();

    let staged = stage(&fixture.request()).unwrap();

    assert_eq!(staged.copied, 1);
    assert_eq!(staged.preserved, vec!["my-dir", "my-thing.md"]);
    assert_eq!(
        fs::read_to_string(fixture.target.join("my-thing.md")).unwrap(),
        "mine\n",
        "a user's own command file must survive an install"
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("my-dir").join("note.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("lock.md")).unwrap(),
        "v2\n",
        "amplihack's own command is still refreshed"
    );
    assert!(!fixture.staging_dir().exists());
    assert!(!fixture.backup_dir().exists());
}

#[test]
fn scratch_directories_live_outside_the_target_parent() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v1\n").unwrap();
    let scan_root = fixture.target.parent().unwrap().to_path_buf();

    stage(&fixture.request()).unwrap();

    let siblings: Vec<String> = fs::read_dir(&scan_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        siblings,
        vec!["amplihack".to_string()],
        "every subdirectory of the scan root is a command namespace, so a \
         scratch dir there surfaces phantom /amplihack.staging:* commands"
    );
}

/// A failed transform returns through `?`; the scratch dir must not survive it.
#[test]
fn a_failed_copy_leaves_no_orphan_scratch_directory() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v1\n").unwrap();

    let error = stage_command_files(&fixture.request(), |_path, _body| {
        anyhow::bail!("transform exploded")
    })
    .expect_err("the transform failure must propagate");

    assert!(format!("{error:#}").contains("transform exploded"));
    assert!(
        !fixture.staging_dir().exists(),
        "an orphan .staging dir is reachable from every `?` in the copy loop"
    );
    assert!(!fixture.target.exists());
}

/// A crash between the two renames leaves no target and the previous directory
/// in `.old`. The next run must put it back rather than orphan it forever —
/// the old code's "target missing" branch never touched the backup at all.
#[test]
fn an_interrupted_swap_is_recovered_on_the_next_run() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v2\n").unwrap();
    let backup = fixture.backup_dir();
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("my-thing.md"), "mine\n").unwrap();
    fs::write(backup.join("lock.md"), "v1\n").unwrap();

    let staged = stage(&fixture.request()).unwrap();

    assert_eq!(staged.copied, 1);
    assert_eq!(staged.preserved, vec!["my-thing.md"]);
    assert_eq!(
        fs::read_to_string(fixture.target.join("my-thing.md")).unwrap(),
        "mine\n"
    );
    assert!(!fixture.backup_dir().exists());
    assert!(!fixture.staging_dir().exists());
}

#[test]
fn transform_is_applied_to_every_staged_file() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("a.md"), "body\n").unwrap();
    fs::write(fixture.source.join("b.md"), "body\n").unwrap();

    let staged = stage_command_files(&fixture.request(), |path, body| {
        Ok(format!(
            "{}:{body}",
            path.file_stem().unwrap().to_str().unwrap()
        ))
    })
    .unwrap();

    assert_eq!(staged.copied, 2);
    assert_eq!(
        fs::read_to_string(fixture.target.join("a.md")).unwrap(),
        "a:body\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("b.md")).unwrap(),
        "b:body\n"
    );
}

/// Issue #1954 D4: the previous command set is retired under
/// `releases/.retired/`, not deleted, whichever swap path ran.
#[test]
fn restaging_retires_the_previous_command_set() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v2\n").unwrap();
    fs::create_dir_all(&fixture.target).unwrap();
    fs::write(fixture.target.join("lock.md"), "v1\n").unwrap();

    let mut request = fixture.request();
    request.target_is_owned = true;
    stage(&request).unwrap();

    assert_eq!(
        fs::read_to_string(fixture.target.join("lock.md")).unwrap(),
        "v2\n"
    );
    let retired_dir = fixture.amp.join("releases").join(".retired");
    let retired: Vec<PathBuf> = fs::read_dir(&retired_dir)
        .unwrap_or_else(|e| panic!("{} must exist: {e}", retired_dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(retired.len(), 1, "one retired tree expected: {retired:?}");
    assert!(
        retired[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("-{RETIRE_LABEL}")),
        "unexpected retired name {}",
        retired[0].display()
    );
    assert_eq!(
        fs::read_to_string(retired[0].join("lock.md")).unwrap(),
        "v1\n",
        "the retired tree must be the previous command set"
    );
    assert!(!fixture.staging_dir().exists());
    assert!(!fixture.backup_dir().exists());
}

impl Fixture {
    fn scratch(&self) -> Scratch<'_> {
        Scratch {
            retire_root: &self.amp,
            scratch_root: &self.scratch,
            target: &self.target,
        }
    }

    /// Makes every retire fail: `releases` is a file, so `.retired/` cannot be
    /// created under it.
    fn block_retiring(&self) {
        fs::create_dir_all(&self.amp).unwrap();
        fs::write(self.amp.join("releases"), "not a directory").unwrap();
    }

    /// A previous command set at `path`, as a swap leaves it.
    fn old_set(&self, path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("lock.md"), "v1\n").unwrap();
    }
}

fn scan_with(pids: &'static [u32]) -> impl FnOnce(&[PathBuf]) -> InUse {
    move |candidates: &[PathBuf]| {
        let mut map = std::collections::BTreeMap::new();
        if !pids.is_empty() {
            map.insert(candidates[0].clone(), pids.to_vec());
        }
        InUse::Known(map)
    }
}

/// Crusty round 1 (#2137): a tree that cannot be retired is removed only when
/// the scan completes and names no process.
#[test]
fn an_unretirable_unused_command_set_is_removed() {
    let fixture = Fixture::new();
    fixture.block_retiring();
    let old = fixture.backup_dir();
    fixture.old_set(&old);

    let disposal = dispose_old_commands(&fixture.scratch(), &old, scan_with(&[]));

    assert_eq!(disposal, Disposal::Removed);
    assert!(!old.exists());
}

/// Crusty round 1 (#2137): a tree that cannot be retired and that a process
/// uses is kept, under a name that does not block the next swap.
#[test]
fn an_unretirable_command_set_in_use_is_kept_aside() {
    let fixture = Fixture::new();
    fixture.block_retiring();
    let old = fixture.staging_dir();
    fixture.old_set(&old);

    let disposal = dispose_old_commands(&fixture.scratch(), &old, scan_with(&[4242]));

    let kept = fixture.scratch.join("amplihack.unretired-1");
    assert_eq!(disposal, Disposal::Kept(kept.clone()));
    assert!(
        !old.exists(),
        "the fixed scratch name is free for the next swap"
    );
    assert_eq!(fs::read_to_string(kept.join("lock.md")).unwrap(), "v1\n");
}

/// An incomplete scan is treated as "in use": nothing is deleted.
#[test]
fn an_unretirable_command_set_is_kept_when_the_scan_is_incomplete() {
    for scan in [InUse::Unknown, InUse::Unsupported] {
        let fixture = Fixture::new();
        fixture.block_retiring();
        let old = fixture.backup_dir();
        fixture.old_set(&old);

        let disposal = dispose_old_commands(&fixture.scratch(), &old, move |_| scan);

        let kept = fixture.scratch.join("amplihack.unretired-1");
        assert_eq!(disposal, Disposal::Kept(kept.clone()));
        assert_eq!(fs::read_to_string(kept.join("lock.md")).unwrap(), "v1\n");
    }
}

/// A tree already kept aside is checked again where it is, not renamed again.
#[test]
fn a_kept_command_set_still_in_use_stays_where_it_is() {
    let fixture = Fixture::new();
    fixture.block_retiring();
    let kept = fixture.scratch.join("amplihack.unretired-1");
    fixture.old_set(&kept);

    let disposal = dispose_old_commands(&fixture.scratch(), &kept, scan_with(&[4242]));

    assert_eq!(disposal, Disposal::Kept(kept.clone()));
    assert_eq!(fs::read_to_string(kept.join("lock.md")).unwrap(), "v1\n");
    assert!(!fixture.scratch.join("amplihack.unretired-2").exists());
}

/// A leftover `.staging` tree from a run that stopped after the exchange holds
/// the previously live set: it is retired, never deleted by the scratch guard.
#[test]
fn a_leftover_staging_tree_is_retired_not_deleted() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("lock.md"), "v2\n").unwrap();
    fixture.old_set(&fixture.staging_dir());

    stage(&fixture.request()).unwrap();

    assert_eq!(
        fs::read_to_string(fixture.target.join("lock.md")).unwrap(),
        "v2\n"
    );
    let retired_dir = fixture.amp.join("releases").join(".retired");
    let retired: Vec<PathBuf> = fs::read_dir(&retired_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(retired.len(), 1, "{retired:?}");
    assert_eq!(
        fs::read_to_string(retired[0].join("lock.md")).unwrap(),
        "v1\n"
    );
    assert!(!fixture.staging_dir().exists());
}
