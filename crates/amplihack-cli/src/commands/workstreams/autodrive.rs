//! Reading auto-drive state (issue #1710) through the layout its bash writers
//! use: `amplifier-bundle/tools/autodrive_layout.sh` is compiled in here and
//! sourced by `autodrive_state.sh` and `autodrive_loop.sh`, so the names are
//! defined once.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, bail};

use super::meta::{self, DirCheck};

const LAYOUT_SH: &str = include_str!("../../../../../amplifier-bundle/tools/autodrive_layout.sh");
/// State files larger than this are not read.
const MAX_STATE_BYTES: u64 = 256 * 1024;
/// Directory entries looked at when counting rounds.
const MAX_ENTRIES: usize = 4096;
/// Rounds counted per loop at most.
const MAX_ROUNDS: u32 = 512;

/// The auto-drive state layout, from `autodrive_layout.sh`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout {
    pub state_subdir: String,
    pub phases_file: String,
    pub phase_build: String,
    pub phase_crusty_loop: String,
    pub phase_merge_ready_loop: String,
    pub phase_merged: String,
    pub loop_crusty: String,
    pub loop_merge_ready: String,
    pub latest_suffix: String,
    pub records_suffix: String,
    pub round_infix: String,
}

static LAYOUT: LazyLock<Layout> = LazyLock::new(|| {
    parse_layout(LAYOUT_SH).expect("the compiled-in autodrive_layout.sh is valid (tested)")
});

pub(crate) fn layout() -> &'static Layout {
    &LAYOUT
}

/// Parse the layout file: comments, blank lines, and exactly one plain
/// `AUTODRIVE_<NAME>=<value>` line per known name, `value` in `[a-z.-]+`.
pub(crate) fn parse_layout(text: &str) -> Result<Layout> {
    const NAMES: [&str; 11] = [
        "AUTODRIVE_STATE_SUBDIR",
        "AUTODRIVE_PHASES_FILE",
        "AUTODRIVE_PHASE_BUILD",
        "AUTODRIVE_PHASE_CRUSTY_LOOP",
        "AUTODRIVE_PHASE_MERGE_READY_LOOP",
        "AUTODRIVE_PHASE_MERGED",
        "AUTODRIVE_LOOP_CRUSTY",
        "AUTODRIVE_LOOP_MERGE_READY",
        "AUTODRIVE_LATEST_SUFFIX",
        "AUTODRIVE_RECORDS_SUFFIX",
        "AUTODRIVE_ROUND_INFIX",
    ];
    let mut values: [Option<String>; 11] = Default::default();
    for (n, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            bail!("autodrive_layout.sh:{}: not a NAME=value line", n + 1);
        };
        let Some(i) = NAMES.iter().position(|k| *k == name) else {
            bail!("autodrive_layout.sh:{}: unknown name", n + 1);
        };
        if value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'-')
        {
            bail!(
                "autodrive_layout.sh:{}: the value is not plain [a-z.-]+",
                n + 1
            );
        }
        if values[i].replace(value.to_string()).is_some() {
            bail!("autodrive_layout.sh:{}: {name} is set twice", n + 1);
        }
    }
    let mut take = |name: &str| -> Result<String> {
        let i = NAMES.iter().position(|k| *k == name).expect("a known name");
        match values[i].take() {
            Some(v) => Ok(v),
            None => bail!("autodrive_layout.sh: {name} is missing"),
        }
    };
    Ok(Layout {
        state_subdir: take("AUTODRIVE_STATE_SUBDIR")?,
        phases_file: take("AUTODRIVE_PHASES_FILE")?,
        phase_build: take("AUTODRIVE_PHASE_BUILD")?,
        phase_crusty_loop: take("AUTODRIVE_PHASE_CRUSTY_LOOP")?,
        phase_merge_ready_loop: take("AUTODRIVE_PHASE_MERGE_READY_LOOP")?,
        phase_merged: take("AUTODRIVE_PHASE_MERGED")?,
        loop_crusty: take("AUTODRIVE_LOOP_CRUSTY")?,
        loop_merge_ready: take("AUTODRIVE_LOOP_MERGE_READY")?,
        latest_suffix: take("AUTODRIVE_LATEST_SUFFIX")?,
        records_suffix: take("AUTODRIVE_RECORDS_SUFFIX")?,
        round_infix: take("AUTODRIVE_ROUND_INFIX")?,
    })
}

/// `autodrive_state_root`: `$AMPLIHACK_STATE_DIR/<subdir>` when that is an
/// absolute path, else `<home>/.amplihack/state/<subdir>`.
pub(crate) fn state_root(state_dir: Option<&OsStr>, home: &Path) -> PathBuf {
    let base = state_dir
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".amplihack").join("state"));
    base.join(&layout().state_subdir)
}

/// `autodrive_state_key`: `<origin minus .git>/<ident>` (origin `local` when
/// there is none), with every byte outside `A-Za-z0-9._-` replaced by `_`,
/// byte by byte as `tr` does.
pub(crate) fn state_key(origin: Option<&str>, ident: &str) -> String {
    let slug = origin.unwrap_or("local");
    let slug = slug.strip_suffix(".git").unwrap_or(slug);
    format!("{slug}/{ident}")
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                char::from(b)
            } else {
                '_'
            }
        })
        .collect()
}

/// What one run's state dir says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AutoDriveState {
    /// Completed phases, in the order recorded.
    pub phases: Vec<String>,
    pub crusty_rounds: u32,
    pub merge_ready_rounds: u32,
    /// `concern_count` of the latest crusty round.
    pub concerns: Option<u32>,
}

/// Read the state dir `dir`. `None` when it is missing, a symlink, not owned
/// by this user, or writable by group or others; files in it that are
/// symlinks or not private are skipped.
pub(crate) fn read(dir: &Path) -> Option<AutoDriveState> {
    if meta::check_dir(dir) != DirCheck::Private {
        return None;
    }
    let l = layout();
    let phases = meta::read_private(&dir.join(&l.phases_file), MAX_STATE_BYTES)
        .map(|body| {
            String::from_utf8_lossy(&body)
                .lines()
                .filter_map(|line| line.split('\t').next())
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .take(MAX_ENTRIES)
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    let concerns = meta::read_private(
        &dir.join(format!("{}{}", l.loop_crusty, l.latest_suffix)),
        MAX_STATE_BYTES,
    )
    .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
    .and_then(|v| v.get("concern_count")?.as_u64())
    .and_then(|n| u32::try_from(n).ok());
    Some(AutoDriveState {
        phases,
        crusty_rounds: rounds(dir, &names, &l.loop_crusty),
        merge_ready_rounds: rounds(dir, &names, &l.loop_merge_ready),
        concerns,
    })
}

/// The number of `<loop>-round-<N>.json` records, else the non-blank rows of
/// `<loop>-records.tsv`.
fn rounds(dir: &Path, names: &[String], loop_name: &str) -> u32 {
    let l = layout();
    let prefix = format!("{loop_name}{}", l.round_infix);
    let numbered: BTreeSet<u32> = names
        .iter()
        .filter_map(|n| n.strip_prefix(&prefix)?.strip_suffix(".json"))
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|n| n.parse().ok())
        .take(MAX_ROUNDS as usize)
        .collect();
    if !numbered.is_empty() {
        return numbered.len() as u32;
    }
    meta::read_private(
        &dir.join(format!("{loop_name}{}", l.records_suffix)),
        MAX_STATE_BYTES,
    )
    .map(|body| {
        let rows = String::from_utf8_lossy(&body)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        u32::try_from(rows).unwrap_or(MAX_ROUNDS).min(MAX_ROUNDS)
    })
    .unwrap_or(0)
}
