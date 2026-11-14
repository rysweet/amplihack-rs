//! `init`, `record` and `report`: where a recovery directory starts, how a
//! recipe records a handoff when no step failed, and where a handed-off run
//! ends.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::driver::check_recovery_dir;
use super::record::{
    self, Choice, Failure, Record, Source, check_text, validate_caller_step_id, validate_owner,
};
use super::{SCHEMA_VERSION, STEP_RECOVERY_HANDOFF_EXIT, signature};

#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// Check this directory instead of creating one.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// When it creates the directory, note this recipe as its creator, so
    /// only that recipe's `report --remove-if-created-by` removes it.
    #[arg(long)]
    pub created_by: Option<String>,
}

#[derive(clap::Args, Debug)]
pub struct RecordArgs {
    #[arg(long)]
    pub dir: PathBuf,
    #[arg(long)]
    pub step_id: String,
    #[arg(long)]
    pub reason: String,
    #[arg(long)]
    pub owner: String,
    #[arg(long)]
    pub needs: String,
}

#[derive(clap::Args, Debug)]
pub struct ReportArgs {
    #[arg(long)]
    pub dir: PathBuf,
    /// Remove the directory when it holds no record and `init
    /// --created-by` named this recipe. A directory with any other file is
    /// kept, and so is a caller's directory.
    #[arg(long)]
    pub remove_if_created_by: Option<String>,
}

/// `$XDG_STATE_HOME` when set and absolute, else `$HOME/.local/state`.
fn state_home() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        && xdg.is_absolute()
    {
        return Ok(xdg);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .context("HOME is not set to an absolute path")?;
    Ok(home.join(".local").join("state"))
}

#[cfg(unix)]
fn make_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = path.parent().context("recovery directory has no parent")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .with_context(|| format!("cannot create {}", parent.display()))?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .with_context(|| format!("cannot create {}", path.display()))
}

#[cfg(not(unix))]
fn make_private_dir(_: &Path) -> Result<()> {
    anyhow::bail!("step recovery requires Unix")
}

/// Name of the file in which `init` notes the recipe that created the directory.
const CREATED_BY: &str = ".created-by";

fn init_dir(args: InitArgs) -> Result<PathBuf> {
    if let Some(tag) = &args.created_by {
        validate_caller_step_id(tag)?;
    }
    let dir = match args.dir {
        Some(dir) => dir,
        None => {
            let name = uuid::Uuid::new_v4().simple().to_string();
            let dir = state_home()?
                .join("amplihack")
                .join("step-recovery")
                .join(name);
            make_private_dir(&dir)?;
            if let Some(tag) = &args.created_by {
                super::secure_fs::create(
                    super::secure_fs::Scope::Strict,
                    &dir.join(CREATED_BY),
                    tag.as_bytes(),
                )?;
            }
            dir
        }
    };
    check_recovery_dir(&dir)?;
    Ok(dir)
}

/// Prints the checked directory. On any error it warns on stderr, prints an
/// empty line and exits 0: recovery is off for the run, the wrapped steps
/// run unwrapped and fail exactly as they did before recovery existed.
pub fn init(args: InitArgs) -> Result<i32> {
    match init_dir(args) {
        Ok(dir) => println!("{}", dir.display()),
        Err(e) => {
            eprintln!("[step-recovery] WARNING: recovery disabled: {e:#}");
            println!();
        }
    }
    Ok(0)
}

pub fn record(args: RecordArgs) -> Result<i32> {
    validate_caller_step_id(&args.step_id)?;
    check_recovery_dir(&args.dir)?;
    validate_owner(&args.owner)?;
    for (name, value) in [("--reason", &args.reason), ("--needs", &args.needs)] {
        check_text(name, value)?;
    }
    let record = Record {
        schema_version: SCHEMA_VERSION,
        step_id: args.step_id,
        failure: Failure {
            exit_code: None,
            signal: None,
            unavailable_reason: Some("no step failed: required evidence is missing".to_string()),
            signature: signature::signature(None, None, ""),
            output_excerpt: String::new(),
        },
        choice: Choice::Handoff,
        reason: args.reason,
        repair: None,
        alternative: None,
        owner: Some(args.owner),
        needs: Some(args.needs),
        recurrence: false,
        source: Source::Validator,
        recorded_at: record::now_utc(),
    };
    let path = record::append(&args.dir, &record)?;
    eprintln!("[step-recovery] handoff recorded in {}", path.display());
    Ok(0)
}

/// Exit 0 with no handoff, 81 when any record is a handoff. Never prints
/// `output_excerpt`.
pub fn report(args: ReportArgs) -> Result<i32> {
    check_recovery_dir(&args.dir)?;
    let mut handoffs = Vec::new();
    let all = record::read_all(&args.dir)?;
    if let Some(tag) = &args.remove_if_created_by
        && all.is_empty()
        && super::secure_fs::read_optional(&args.dir.join(CREATED_BY), 256)?.as_deref()
            == Some(tag.as_bytes())
    {
        remove_clean_dir(&args.dir);
    }
    for (path, records) in all {
        for r in records.into_iter().filter(|r| r.choice == Choice::Handoff) {
            eprintln!(
                "[step-recovery] HANDOFF step={} owner={} needs={} reason={} record={}",
                r.step_id,
                r.owner.as_deref().unwrap_or_default(),
                r.needs.as_deref().unwrap_or_default(),
                r.reason,
                path.display()
            );
            handoffs.push(serde_json::json!({
                "step_id": r.step_id,
                "owner": r.owner,
                "needs": r.needs,
                "reason": r.reason,
                "source": r.source,
                "record": path.to_string_lossy(),
            }));
        }
    }
    let code = if handoffs.is_empty() {
        0
    } else {
        STEP_RECOVERY_HANDOFF_EXIT
    };
    println!(
        "{}",
        serde_json::json!({ "step_recovery_handoffs": handoffs })
    );
    Ok(code)
}

/// Removes a directory that holds no record: first the creator note and
/// the retry branch note (`build-branch.txt`, written by workflow-worktree),
/// then the directory. Any other file keeps it; the reason goes to stderr.
fn remove_clean_dir(dir: &Path) {
    let note = |name: &str| match std::fs::remove_file(dir.join(name)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    };
    let removed = note("build-branch.txt")
        .and_then(|()| note(CREATED_BY))
        .and_then(|()| std::fs::remove_dir(dir));
    match removed {
        Ok(()) => eprintln!(
            "[step-recovery] removed {}: it held no record",
            dir.display()
        ),
        Err(e) => eprintln!("[step-recovery] kept {}: {e}", dir.display()),
    }
}
