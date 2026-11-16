//! Issue #1619: the shell a recipe step actually runs.
//!
//! Several bundled recipes were at the 400-line brick budget before this
//! issue added one `model:` line per agent step. The project's rule for a
//! full brick is to extract a bash step body into `amplifier-bundle/tools/`
//! and call out to it, never to compress the file, so some step bodies now
//! live in a file beside the recipe.
//!
//! Every test that reads a step's shell to assert something about it has to
//! follow that call-out, or it reads six lines of lookup and concludes that a
//! step with real requirements has none. This is the one place that follows
//! it, so the next extraction does not need to find them all again.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

/// The comment a call-out block starts with, written by the extraction.
const MARKER: &str = "# The body of this step lives in amplifier-bundle/tools/";

/// `inline` itself, or the text of the `amplifier-bundle/tools/` script it
/// calls out to.
///
/// The script must exist: a step that calls a file that is not there would
/// otherwise read as a step with no requirements at all, which is exactly the
/// reading these tests exist to prevent.
pub fn resolved(inline: &str) -> String {
    match script_name(inline) {
        Some(name) => tool_text(&name),
        None => inline.to_string(),
    }
}

/// The script a call-out names, or `None` when the body is inline.
fn script_name(inline: &str) -> Option<String> {
    let line = inline
        .lines()
        .find(|line| line.trim_start().starts_with(MARKER))?;
    Some(
        line.trim_end()
            .trim_end_matches(',')
            .rsplit('/')
            .next()
            .expect("a call-out names its script")
            .to_string(),
    )
}

/// The text of one bundled tool script, read once per process.
fn tool_text(name: &str) -> String {
    static TEXTS: LazyLock<Mutex<HashMap<String, String>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut texts = TEXTS.lock().expect("tool text cache");
    if let Some(text) = texts.get(name) {
        return text.clone();
    }
    let path = tools_dir().join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    texts.insert(name.to_string(), text.clone());
    text
}

fn tools_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .join("amplifier-bundle/tools")
}
