//! Unit tests of the memory module, split by area.

use super::language::*;
use super::scoring::*;
use super::text::*;
use super::*;

pub(super) fn memory(content: &str) -> PromptContextMemory {
    PromptContextMemory {
        content: content.to_string(),
        code_context: None,
    }
}

pub(super) fn agents(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

mod code_spans;
mod english_check;
mod format;
mod scoring;
mod stop_words;
