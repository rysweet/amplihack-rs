//! Issue #1619: the tier vocabulary and the per-harness table.
//!
//! The vocabulary is closed: `fast`, `standard`, `strong`, and no alias. The
//! table maps one `(harness column, tier)` to a model, a reasoning effort,
//! both or neither. The shipped default is compiled in from
//! `../model-tiers.toml`; an instance's file is parsed by the same code and
//! merged over it cell by cell.
//!
//! This module reads no file and no environment variable.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::notice::Notice;
use crate::commands::launch::command::is_claude_compatible_tool;

/// Longest accepted model id, in bytes. `user_config::schema` states this
/// in the requirement it prints, rather than repeating the number.
pub(crate) const MAX_MODEL_ID_BYTES: usize = 128;

/// The one version of the tier-file format amplihack knows.
const VERSION: i64 = 1;

/// The reserved top-level key that is not a harness column.
const STEP_TIERS_KEY: &str = "step_tiers";

/// The reserved key inside a column that is not a tier word: the highest
/// reasoning effort this account's models accept.
const MAX_EFFORT_KEY: &str = "max_effort";

/// What a recipe step declares instead of a model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Fast,
    Standard,
    Strong,
}

impl Tier {
    /// Every tier there is: no fourth and no alias. [`Tier::parse`] and every
    /// diagnostic that lists the words read this, so a word cannot exist in
    /// one place and not another.
    pub(crate) const ALL: [Tier; 3] = [Tier::Fast, Tier::Standard, Tier::Strong];

    /// The tier `raw` names, trimmed and matched case-insensitively, or
    /// `None` when `raw` is anything else — including a concrete model id,
    /// which is forwarded as typed (issue #1527).
    pub(crate) fn parse(raw: &str) -> Option<Tier> {
        let word = raw.trim();
        Tier::ALL
            .into_iter()
            .find(|tier| tier.as_str().eq_ignore_ascii_case(word))
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Tier::Fast => "fast",
            Tier::Standard => "standard",
            Tier::Strong => "strong",
        }
    }
}

/// A reasoning effort amplihack is willing to put on a command line.
///
/// One union of words for every harness, ordered weakest to strongest: the
/// derived `Ord` is what makes a column's `max_effort` lower an effort
/// correctly, so a new variant belongs in its place in the ranking rather
/// than at the end of the list. Which of the words a given harness accepts is
/// [`Effort::accepted_in`], not this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Claude Code's own vocabulary, as `claude --help` states it on 2.1.329.
/// It has no `minimal`.
const CLAUDE_EFFORTS: [Effort; 5] = [
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::Xhigh,
    Effort::Max,
];

/// Codex's own vocabulary. It has no `max`.
const CODEX_EFFORTS: [Effort; 5] = [
    Effort::Minimal,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::Xhigh,
];

impl Effort {
    /// Every reasoning effort amplihack will write on a command line. Argv
    /// safety, not a claim about what any one harness accepts: the value
    /// becomes an argv token, as the right-hand side of
    /// `-c model_reasoning_effort=…` or as Claude Code's `--effort` value.
    pub(crate) const ALL: [Effort; 6] = [
        Effort::Minimal,
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::Xhigh,
        Effort::Max,
    ];

    /// The words `column` itself accepts.
    ///
    /// A column amplihack writes no effort onto accepts the union: nothing it
    /// names can be wrong on a command line, so refusing a word there would
    /// buy nothing and cost an operator a working cell.
    pub(crate) fn accepted_in(column: &str) -> &'static [Effort] {
        match column {
            "claude" => &CLAUDE_EFFORTS,
            "codex" => &CODEX_EFFORTS,
            _ => &Effort::ALL,
        }
    }

    /// Whether `column` has a launch control for reasoning effort, so a cell's
    /// effort reaches the harness as its own argument.
    ///
    /// Read by the resolver and by the injection dispatch alike, so a column
    /// cannot be validated against a vocabulary it is never written with, nor
    /// written with a word it was never validated against.
    pub(crate) fn has_effort_control(column: &str) -> bool {
        matches!(column, "claude" | "codex")
    }

    /// The effort `raw` names exactly, ignoring ASCII case, or `None`.
    ///
    /// Exact: no trimming, no prefix matching. The value becomes an argv
    /// token, so a word carrying a newline or a trailing space is reported
    /// as a typo, not corrected.
    pub(crate) fn parse(raw: &str) -> Option<Effort> {
        Effort::ALL
            .into_iter()
            .find(|effort| effort.as_str().eq_ignore_ascii_case(raw))
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
            Effort::Max => "max",
        }
    }
}

/// One cell of the table: what a tier means to one harness.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The model to request, or `None` to let the harness choose.
    pub(crate) model: Option<String>,
    /// The reasoning effort to request, where the harness has one.
    pub(crate) effort: Option<Effort>,
}

/// A whole mapping: harness columns, their cells, and the per-step tier
/// overrides that travel in the same file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Table {
    columns: BTreeMap<String, BTreeMap<Tier, Entry>>,
    /// Each column's declared `max_effort`, which only ever lowers a tier's.
    ceilings: BTreeMap<String, Effort>,
    step_tiers: BTreeMap<String, Tier>,
}

impl Table {
    /// The mapping amplihack ships, parsed once from the compiled-in file.
    ///
    /// A parse failure here is a build error, not an operator error.
    pub(crate) fn shipped() -> &'static Table {
        static SHIPPED: OnceLock<Table> = OnceLock::new();
        SHIPPED.get_or_init(|| {
            let (table, notices) = Table::parse_override(SHIPPED_TEXT).unwrap_or_else(|reason| {
                panic!("the model tier table amplihack ships does not parse: {reason}")
            });
            assert!(
                notices.is_empty(),
                "the model tier table amplihack ships has diagnostics: {notices:?}"
            );
            table
        })
    }

    /// The cell for `column` and `tier`, or `None` when the column has none.
    pub(crate) fn entry(&self, column: &str, tier: Tier) -> Option<&Entry> {
        self.columns.get(column)?.get(&tier)
    }

    /// Whether this table knows `column` at all. A column it does not know
    /// and a column with no cells are different: the first is a harness
    /// nobody has configured, the second is one somebody emptied.
    pub(crate) fn has_column(&self, column: &str) -> bool {
        self.columns.contains_key(column)
    }

    /// The per-step tier overrides this file declares.
    pub(crate) fn step_tiers(&self) -> &BTreeMap<String, Tier> {
        &self.step_tiers
    }

    /// The highest reasoning effort `column` accepts, when it declares one.
    ///
    /// A declaration about the operator's own account, not something
    /// amplihack detected: learning that a model refuses an effort needs a
    /// live call. The shipped mapping declares none.
    pub(crate) fn max_effort(&self, column: &str) -> Option<Effort> {
        self.ceilings.get(column).copied()
    }

    /// This table laid over `base`, cell by cell. A cell this table names
    /// replaces `base`'s, so an empty cell clears a shipped model.
    pub(crate) fn merged_over(&self, base: &Table) -> Table {
        let mut out = base.clone();
        for (column, cells) in &self.columns {
            let target = out.columns.entry(column.clone()).or_default();
            for (tier, entry) in cells {
                target.insert(*tier, entry.clone());
            }
        }
        for (column, effort) in &self.ceilings {
            out.ceilings.insert(column.clone(), *effort);
        }
        for (step, tier) in &self.step_tiers {
            out.step_tiers.insert(step.clone(), *tier);
        }
        out
    }

    /// Parse one tier file's text.
    ///
    /// `Err` is a reason the whole file is refused and the layer below
    /// applies: malformed TOML, or an unknown version. The reason never
    /// quotes the file's contents. Everything else is a [`Notice`]: the entry
    /// it names is dropped and the rest of the file still applies.
    pub(crate) fn parse_override(text: &str) -> Result<(Table, Vec<Notice>), String> {
        let document: toml::Table =
            toml::from_str(text).map_err(|_| "is not valid TOML; ignored".to_string())?;
        match document.get("version").and_then(toml::Value::as_integer) {
            Some(VERSION) => {}
            Some(other) => {
                return Err(format!(
                    "declares version {other}, which this amplihack does not know; ignored"
                ));
            }
            None => {
                return Err(format!("does not declare `version = {VERSION}`; ignored"));
            }
        }

        let mut table = Table::default();
        let mut notices = Vec::new();
        for (key, value) in &document {
            if key == "version" {
                continue;
            }
            let Some(inner) = value.as_table() else {
                notices.push(Notice::NotATable { key: key.clone() });
                continue;
            };
            if key == STEP_TIERS_KEY {
                read_step_tiers(inner, &mut table.step_tiers, &mut notices);
                continue;
            }
            let cells = table.columns.entry(key.clone()).or_default();
            for (word, cell) in inner {
                // Reserved, and matched before `Tier::parse`, so it is never
                // read as a tier word nor reported as an unknown one. The
                // reservation is per-position: inside `[step_tiers]` the same
                // spelling is an ordinary step id.
                if word == MAX_EFFORT_KEY {
                    read_ceiling(key, cell, &mut table.ceilings, &mut notices);
                    continue;
                }
                let Some(tier) = Tier::parse(word) else {
                    notices.push(Notice::UnknownTier {
                        column: key.clone(),
                        word: word.clone(),
                    });
                    continue;
                };
                match read_cell(key, tier, cell, &mut notices) {
                    Some(entry) => {
                        cells.insert(tier, entry);
                    }
                    None => continue,
                }
            }
        }
        Ok((table, notices))
    }
}

/// The compiled-in shipped mapping.
const SHIPPED_TEXT: &str = include_str!("../model-tiers.toml");

/// Read one column's `max_effort`. A value that is not one of the column's
/// own words leaves the column with no ceiling: applying some default cap on
/// a typo would silently change what every step of that column runs at.
///
/// The same membership test as [`read_cell`], so a word that could never be
/// written onto that column's command line cannot enter the `min(cap)`
/// comparison either.
fn read_ceiling(
    column: &str,
    value: &toml::Value,
    out: &mut BTreeMap<String, Effort>,
    notices: &mut Vec<Notice>,
) {
    let raw = value.as_str().unwrap_or_default();
    match accepted_effort(column, raw) {
        Some(effort) => {
            out.insert(column.to_string(), effort);
        }
        None => notices.push(Notice::BadEffort {
            column: column.to_string(),
            tier: MAX_EFFORT_KEY.to_string(),
            value: raw.to_string(),
        }),
    }
}

/// The effort `raw` names, when `column` accepts that word.
///
/// Two gates, not one: `Effort::parse` is argv safety, and
/// `Effort::accepted_in` is the harness's own vocabulary. Codex has no `max`
/// and Claude Code has no `minimal`, so a word good for one column is a typo
/// in the other and is reported where the operator wrote it.
fn accepted_effort(column: &str, raw: &str) -> Option<Effort> {
    let effort = Effort::parse(raw)?;
    Effort::accepted_in(column)
        .contains(&effort)
        .then_some(effort)
}

/// Read a `[step_tiers]` table, dropping and reporting every entry whose
/// value is not one of the three words.
fn read_step_tiers(
    inner: &toml::Table,
    out: &mut BTreeMap<String, Tier>,
    notices: &mut Vec<Notice>,
) {
    for (step, value) in inner {
        let raw = value.as_str().unwrap_or_default();
        match Tier::parse(raw) {
            Some(tier) => {
                out.insert(step.clone(), tier);
            }
            None => notices.push(Notice::BadStepTier {
                step: step.clone(),
                value: raw.to_string(),
            }),
        }
    }
}

/// Read one cell. `None` means the cell is dropped altogether, so the layer
/// below keeps its value: that is what a model amplihack will not write must
/// do, or a rejected id would clear a good shipped one.
fn read_cell(
    column: &str,
    tier: Tier,
    value: &toml::Value,
    notices: &mut Vec<Notice>,
) -> Option<Entry> {
    let Some(fields) = value.as_table() else {
        notices.push(Notice::NotATable {
            key: format!("{column}.{}", tier.as_str()),
        });
        return None;
    };
    let mut entry = Entry::default();
    for (field, item) in fields {
        match field.as_str() {
            "model" => {
                let raw = item.as_str().unwrap_or_default();
                if let Err(reason) = validate_model_id(raw) {
                    notices.push(Notice::RejectedModel {
                        column: column.to_string(),
                        tier: tier.as_str().to_string(),
                        reason,
                    });
                    return None;
                }
                entry.model = Some(raw.to_string());
            }
            "effort" => {
                let raw = item.as_str().unwrap_or_default();
                // A refused effort drops the effort alone, never the cell:
                // the model beside it is still good, and clearing it would
                // take a working model away over a mistyped word.
                match accepted_effort(column, raw) {
                    Some(effort) => entry.effort = Some(effort),
                    None => notices.push(Notice::BadEffort {
                        column: column.to_string(),
                        tier: tier.as_str().to_string(),
                        value: raw.to_string(),
                    }),
                }
            }
            other => notices.push(Notice::UnknownField {
                column: column.to_string(),
                tier: tier.as_str().to_string(),
                field: other.to_string(),
            }),
        }
    }
    Some(entry)
}

/// The harness column `binary_name` reads.
///
/// Every Claude-compatible tool shares one column: they take the same model
/// ids, and splitting them would mean four copies of one cell.
pub(crate) fn column_for(binary_name: &str) -> String {
    if is_claude_compatible_tool(binary_name) {
        return "claude".to_string();
    }
    binary_name.to_string()
}

/// The one check every model string amplihack can emit crosses, from a tier
/// file, `model.force` or `AMPLIHACK_DEFAULT_MODEL`.
///
/// The value lands as the argv token right after `--model`, and both Codex and
/// Claude carry single-token sandbox-bypass flags, so a value that could act
/// as another argument is refused rather than written. Checked on the raw
/// input, before trimming: a leading space would otherwise hide a leading dash
/// from a later reader.
pub(crate) fn validate_model_id(raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err("is empty".to_string());
    }
    if raw.len() > MAX_MODEL_ID_BYTES {
        return Err(format!("is longer than {MAX_MODEL_ID_BYTES} bytes"));
    }
    if raw.starts_with('-') {
        return Err("begins with `-`, so it could act as another argument".to_string());
    }
    if !raw.chars().all(is_model_id_char) {
        return Err(
            "has a character outside letters, digits and `.`, `_`, `-`, `[`, `]`, `@`, `:`"
                .to_string(),
        );
    }
    Ok(())
}

/// The characters a model id may use. Admits `claude-opus-5[1m]` and
/// `gpt-6.1-sol:high`, and no whitespace, control character or anything a
/// shell or command line would read as structure.
fn is_model_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']' | '@' | ':')
}
