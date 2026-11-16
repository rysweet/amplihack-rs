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

/// The only three tier words. There is no fourth and no alias: one set, used
/// in recipes, in config, in the documentation and in the tests.
pub(crate) const TIER_WORDS: [&str; 3] = ["fast", "standard", "strong"];

/// The reasoning-effort values amplihack will write on a command line. This
/// is argv safety, not a claim about what the installed harness accepts: the
/// value becomes the right-hand side of `-c model_reasoning_effort=…`.
pub(crate) const ACCEPTED_EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];

/// Longest accepted model id, in bytes.
const MAX_MODEL_ID_BYTES: usize = 128;

/// The one version of the tier-file format amplihack knows.
const VERSION: i64 = 1;

/// The reserved top-level key that is not a harness column.
const STEP_TIERS_KEY: &str = "step_tiers";

/// What a recipe step declares instead of a model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Fast,
    Standard,
    Strong,
}

impl Tier {
    /// The tier `raw` names, trimmed and matched case-insensitively, or
    /// `None` when `raw` is anything else — including a concrete model id,
    /// which is forwarded as typed (issue #1527).
    pub(crate) fn parse(raw: &str) -> Option<Tier> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "fast" => Some(Tier::Fast),
            "standard" => Some(Tier::Standard),
            "strong" => Some(Tier::Strong),
            _ => None,
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Effort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl Effort {
    pub(crate) fn parse(raw: &str) -> Option<Effort> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "minimal" => Some(Effort::Minimal),
            "low" => Some(Effort::Low),
            "medium" => Some(Effort::Medium),
            "high" => Some(Effort::High),
            "xhigh" => Some(Effort::Xhigh),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
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
    step_tiers: BTreeMap<String, Tier>,
}

impl Table {
    /// The mapping amplihack ships, parsed once from the compiled-in file.
    ///
    /// A parse failure here is a build error, not an operator error: the file
    /// is inside the binary and `tests_table.rs` parses it.
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

    /// This table laid over `base`, cell by cell. A column `base` does not
    /// have is added whole; a cell this table names replaces `base`'s, so an
    /// empty cell is how an instance clears a shipped model.
    pub(crate) fn merged_over(&self, base: &Table) -> Table {
        let mut out = base.clone();
        for (column, cells) in &self.columns {
            let target = out.columns.entry(column.clone()).or_default();
            for (tier, entry) in cells {
                target.insert(*tier, entry.clone());
            }
        }
        for (step, tier) in &self.step_tiers {
            out.step_tiers.insert(step.clone(), *tier);
        }
        out
    }

    /// Parse one tier file's text.
    ///
    /// `Err` is a reason the whole file is refused and the layer below
    /// applies: malformed TOML, or a version amplihack does not know. The
    /// reason never quotes the file's contents. Everything else is a
    /// [`Notice`]: the entry it names is dropped and the rest of the file
    /// still applies, so one typo cannot stop a run of a hundred steps.
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
                match Effort::parse(raw) {
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

/// The one check every model string amplihack can emit crosses, whether it
/// came from a tier file, from `model.force` or from `AMPLIHACK_FORCE_MODEL`.
///
/// The value lands as the argv token right after `--model`, and both Codex
/// and Claude carry single-token sandbox-bypass flags, so a value that could
/// act as another argument is refused rather than written. Checked on the raw
/// input, before any trimming: a leading space would otherwise hide a leading
/// dash from a later reader.
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

/// The characters a model id may use. Admits `claude-opus-5[1m]`,
/// `claude-sonnet-5.5` and `gpt-6.1-sol:high`, and admits no whitespace, no
/// control character and nothing a shell or a command line would read as
/// structure.
fn is_model_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']' | '@' | ':')
}
