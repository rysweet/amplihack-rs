//! `amplihack sessions archive`: moves each old session that is not live
//! into `<DEST>/<project>/<id>.tar.gz`, records it in the manifest, then
//! removes the originals. See docs/reference/sessions-command.md.
//!
//! Per session, in order: write a temporary archive from descriptors opened
//! without following symlinks, sync it, read all of it back, hard-link it to
//! a free final name, sync the directory, check the session is unchanged and
//! not live, append and sync the manifest line, and only then remove the
//! originals. Any failure before removal keeps the originals.

use super::config::Opts;
use super::fmt::{esc, gib, kind_word, remove_failed, thousands};
use super::live::{self, LiveSet, Probe, SystemProbe};
use super::scan::{self, Session};
use super::store::{Dest, Entry, LockError, contains_or_overlaps};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const DAY_SECS: u64 = 86_400;
/// The live set is rebuilt before a removal when older than this.
const LIVE_MAX_AGE: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub archived: u64,
    pub bytes: u64,
    pub live_kept: u64,
    pub failed: u64,
}

#[derive(Debug)]
pub enum ArchiveError {
    LockHeld(PathBuf),
    DestOverlapsProjects(PathBuf, PathBuf),
    NotPrivate(PathBuf),
    Setup(PathBuf, io::ErrorKind),
    LiveUnreadable(PathBuf, io::ErrorKind),
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LockHeld(p) => write!(
                f,
                "another sessions archive run is in progress (lock: {})",
                esc(p)
            ),
            Self::DestOverlapsProjects(d, r) => {
                write!(
                    f,
                    "destination {} overlaps {}; nothing changed",
                    esc(d),
                    esc(r)
                )
            }
            Self::NotPrivate(p) => write!(
                f,
                "destination {} is not private (must be your own directory, not writable by group or others)",
                esc(p)
            ),
            Self::Setup(p, k) => write!(f, "cannot prepare {} ({})", esc(p), kind_word(*k)),
            Self::LiveUnreadable(p, k) => write!(
                f,
                "cannot read live sessions in {} ({}); nothing archived",
                esc(p),
                kind_word(*k)
            ),
        }
    }
}

impl From<LockError> for ArchiveError {
    fn from(e: LockError) -> Self {
        match e {
            LockError::Held(p) => Self::LockHeld(p),
            LockError::Setup(p, k) => Self::Setup(p, k),
        }
    }
}

/// `now − days × 86400`; `None` when that is not representable.
pub fn cutoff(now: SystemTime, days: u32) -> Option<SystemTime> {
    now.checked_sub(Duration::from_secs(u64::from(days) * DAY_SECS))
}

/// Old when the newest part is at or before the cutoff. Without a cutoff
/// nothing is old.
pub fn is_old(newest: SystemTime, cutoff: Option<SystemTime>) -> bool {
    cutoff.is_some_and(|c| newest <= c)
}

/// `<id>.tar.gz`, `<id>.<ts>.tar.gz`, then `<id>.<ts>-1.tar.gz` to `-9`.
pub fn archive_names(id: &str, ts: &str) -> Vec<String> {
    let mut names = vec![format!("{id}.tar.gz"), format!("{id}.{ts}.tar.gz")];
    names.extend((1..=9).map(|k| format!("{id}.{ts}-{k}.tar.gz")));
    names
}

pub fn summary(r: &Report, days: u32, apply: bool) -> String {
    let (n, size, live) = (thousands(r.archived), gib(r.bytes), thousands(r.live_kept));
    if apply {
        format!(
            "archived {n} sessions, {size} GiB (older than {days} days; {live} live sessions kept; {} failed)",
            thousands(r.failed)
        )
    } else {
        format!(
            "would archive {n} sessions, {size} GiB (older than {days} days; {live} live sessions kept); dry run, nothing changed (use --apply)"
        )
    }
}

/// The run with the system probe, as the CLI and the rotator's daily run
/// use it: the summary line or the error line.
pub fn run_default(
    opts: &Opts,
    profile_root: &Path,
    warn: &mut dyn FnMut(&str),
) -> Result<String, String> {
    run(opts, profile_root, &SystemProbe, warn)
        .map(|r| summary(&r, opts.days, opts.apply))
        .map_err(|e| e.to_string())
}

/// The live set and when it was built.
struct Live<'a> {
    set: LiveSet,
    built: Instant,
    home: &'a Path,
    profile_root: &'a Path,
    probe: &'a dyn Probe,
}

impl Live<'_> {
    fn refresh(&mut self) -> Result<(), String> {
        if self.built.elapsed() > LIVE_MAX_AGE {
            self.set = live::build(self.home, self.profile_root, self.probe)
                .map_err(|(_, k)| format!("cannot read live sessions ({})", kind_word(k)))?;
            self.built = Instant::now();
        }
        Ok(())
    }
}

/// One archive run. Skipped and failed sessions are reported through
/// `warn`; setup problems that stop the run are the error.
pub fn run(
    opts: &Opts,
    profile_root: &Path,
    probe: &dyn Probe,
    warn: &mut dyn FnMut(&str),
) -> Result<Report, ArchiveError> {
    let root = scan::projects_root(&opts.home);
    if contains_or_overlaps(&opts.dest, &root) {
        return Err(ArchiveError::DestOverlapsProjects(opts.dest.clone(), root));
    }
    match fs::metadata(&root) {
        Ok(_) => {}
        // No projects root: zero sessions, and DEST is not touched.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Report::default()),
        Err(e) => return Err(ArchiveError::Setup(root, e.kind())),
    }
    let dest = Dest::new(opts.dest.clone());
    let _lock = if opts.apply {
        dest.ensure()
            .map_err(|e| ArchiveError::Setup(dest.root.clone(), e.kind()))?;
        if !dest
            .check_private()
            .map_err(|e| ArchiveError::Setup(dest.root.clone(), e.kind()))?
        {
            return Err(ArchiveError::NotPrivate(dest.root.clone()));
        }
        let lock = dest.lock()?;
        for (p, k) in dest.clean_tmp() {
            warn(&format!("cannot remove {} ({})", esc(p), kind_word(k)));
        }
        Some(lock)
    } else {
        None
    };
    let mut live = Live {
        set: live::build(&opts.home, profile_root, probe)
            .map_err(|(p, k)| ArchiveError::LiveUnreadable(p, k))?,
        built: Instant::now(),
        home: &opts.home,
        profile_root,
        probe,
    };
    live.set.report_ignored(warn);
    let canonical = root
        .canonicalize()
        .map_err(|e| ArchiveError::Setup(root.clone(), e.kind()))?;
    let (sessions, skipped) =
        scan::scan(&canonical).map_err(|e| ArchiveError::Setup(root.clone(), e.kind()))?;
    for (project, id, why) in skipped {
        warn(&session_line("skipped", &project, &id, &why.reason()));
    }
    let limit = cutoff(SystemTime::now(), opts.days);
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let mut report = Report::default();
    for s in sessions.iter().filter(|s| is_old(s.newest_mtime, limit)) {
        if live.set.contains(&s.id) {
            report.live_kept += 1;
            continue;
        }
        if !opts.apply {
            report.archived += 1;
            report.bytes = report.bytes.saturating_add(s.bytes);
            continue;
        }
        match archive_one(&dest, &canonical, s, &ts, &mut live) {
            Ok(()) => {
                report.archived += 1;
                report.bytes = report.bytes.saturating_add(s.bytes);
            }
            Err(reason) => {
                report.failed += 1;
                warn(&session_line("failed", &s.project, &s.id, &reason));
            }
        }
    }
    Ok(report)
}

/// `skipped <project>/<id>: <reason>` or `failed …`.
fn session_line(verb: &str, project: &str, id: &str, reason: &str) -> String {
    format!("{verb} {}/{}: {reason}", esc(project), esc(id))
}

fn write_failed(e: &io::Error) -> String {
    format!("write failed ({})", kind_word(e.kind()))
}

/// Archives one session; the error is the fixed reason for its `failed`
/// line.
fn archive_one(
    dest: &Dest,
    root: &Path,
    s: &Session,
    ts: &str,
    live: &mut Live<'_>,
) -> Result<(), String> {
    let pdir = dest.project_dir(&s.project).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData => "archive directory not private".to_string(),
        _ => write_failed(&e),
    })?;
    let src = root.join(&s.project);
    let tmp = pdir.join(format!(".{}.tar.gz.tmp-{}", s.id, std::process::id()));
    let staged = write_tmp(&tmp, &src, s).and_then(|()| verify(&tmp, s));
    let name = match staged.and_then(|()| link_free(&tmp, &pdir, &s.id, ts)) {
        Ok(name) => name,
        Err(reason) => {
            // If this fails, the next run's temporary-file clean-up takes it.
            let _ = fs::remove_file(&tmp);
            return Err(reason);
        }
    };
    let final_path = pdir.join(&name);
    let undo = |reason: String| match fs::remove_file(&final_path) {
        Ok(()) => reason,
        Err(_) => format!("{reason}; archive left at {}", esc(&final_path)),
    };
    fs::remove_file(&tmp)
        .and_then(|()| amplihack_utils::secure_files::fsync_dir(&pdir))
        .map_err(|e| undo(write_failed(&e)))?;
    live.refresh().map_err(&undo)?;
    if live.set.contains(&s.id) {
        return Err(undo("now live".into()));
    }
    match scan::describe(&src, &s.project, &s.id) {
        Ok(now) if now.fingerprint == s.fingerprint => {}
        _ => return Err(undo("changed during archive".into())),
    }
    let entry = Entry {
        time: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        session_id: s.id.clone(),
        project: s.project.clone(),
        bytes: s.bytes,
        archive: format!("{}/{name}", s.project),
        parts: s.parts,
    };
    dest.append(&entry)
        .map_err(|e| undo(format!("manifest write failed ({})", kind_word(e.kind()))))?;
    let removed = fs::remove_file(&s.jsonl).map_err(|e| (s.jsonl.clone(), e));
    let removed = removed.and_then(|()| match &s.dir {
        Some(d) => fs::remove_dir_all(d).map_err(|e| (d.clone(), e)),
        None => Ok(()),
    });
    removed.map_err(|(p, e)| remove_failed(e.kind(), &p, &entry.archive))
}

/// Writes the archive from descriptors opened without following symlinks,
/// with headers taken from each open file, then syncs it.
fn write_tmp(tmp: &Path, src: &Path, s: &Session) -> Result<(), String> {
    let out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(tmp)
        .map_err(|e| write_failed(&e))?;
    let mut tar = tar::Builder::new(GzEncoder::new(out, Compression::default()));
    for part in &s.fingerprint {
        let path = src.join(&part.rel);
        let changed = || "changed during archive".to_string();
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| changed())?;
        let meta = file.metadata().map_err(|_| changed())?;
        if meta.is_dir() != part.is_dir
            || meta.ino() != part.ino
            || meta.dev() != part.dev
            || meta.modified().ok() != Some(part.mtime)
        {
            return Err(changed());
        }
        if !meta.is_dir() && (!meta.is_file() || meta.len() != part.len) {
            return Err(changed());
        }
        let mut header = tar::Header::new_gnu();
        header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
        let appended = if part.is_dir {
            tar.append_data(&mut header, &part.rel, io::empty())
        } else {
            tar.append_data(&mut header, &part.rel, (&file).take(part.len))
        };
        appended.map_err(|e| write_failed(&e))?;
    }
    let file = tar
        .into_inner()
        .and_then(GzEncoder::finish)
        .map_err(|e| write_failed(&e))?;
    file.sync_all().map_err(|e| write_failed(&e))
}

/// Reads the whole archive back: member paths and kinds in plan order,
/// the count, the size sum, every byte of data, and the gzip trailer.
pub(super) fn verify(tmp: &Path, s: &Session) -> Result<(), String> {
    let mismatch = || "verify mismatch".to_string();
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(tmp)
        .map_err(|e| write_failed(&e))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    let mut plan = s.fingerprint.iter();
    let (mut count, mut bytes) = (0u64, 0u64);
    for entry in archive.entries().map_err(|_| mismatch())? {
        let mut entry = entry.map_err(|_| mismatch())?;
        let want = plan.next().ok_or_else(mismatch)?;
        let kind = entry.header().entry_type();
        let path_ok = entry.path().is_ok_and(|p| p.as_ref() == want.rel.as_path());
        let kind_ok = if want.is_dir {
            kind.is_dir()
        } else {
            kind.is_file()
        };
        if !path_ok || !kind_ok {
            return Err(mismatch());
        }
        let read = io::copy(&mut entry, &mut io::sink()).map_err(|_| mismatch())?;
        if read != want.len {
            return Err(mismatch());
        }
        count += 1;
        bytes = bytes.saturating_add(read);
    }
    if count != s.parts || bytes != s.bytes {
        return Err(mismatch());
    }
    // Drain to EOF so the decoder checks the CRC and size trailer.
    io::copy(&mut archive.into_inner(), &mut io::sink()).map_err(|_| mismatch())?;
    Ok(())
}

/// Hard-links `tmp` to the first free name; never replaces anything.
fn link_free(tmp: &Path, pdir: &Path, id: &str, ts: &str) -> Result<String, String> {
    for name in archive_names(id, ts) {
        match fs::hard_link(tmp, pdir.join(&name)) {
            Ok(()) => return Ok(name),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(write_failed(&e)),
        }
    }
    Err("no free archive name".into())
}

#[cfg(test)]
#[path = "tests_archive_seams.rs"]
mod tests_seams;
