//! Attack pattern definitions for prompt injection detection.
//!
//! 19 patterns across 8 categories matching the Python XPIA implementation.

use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, ops::Range};

/// Category of attack pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternCategory {
    PromptOverride,
    InstructionInjection,
    ContextManipulation,
    DataExfiltration,
    SystemEscape,
    RoleHijacking,
    EncodingBypass,
    ChainAttacks,
}

/// Individual attack pattern definition.
#[derive(Debug, Clone)]
pub struct AttackPattern {
    pub id: &'static str,
    pub name: &'static str,
    pub category: PatternCategory,
    pub severity: &'static str,
    pub description: &'static str,
    pub mitigation: &'static str,
    matcher: PatternMatcher,
}

#[derive(Debug, Clone)]
enum PatternMatcher {
    Regex(regex::Regex),
    LengthThreshold(usize),
    ToolContent(regex::Regex),
    InstructionChain,
}

impl AttackPattern {
    /// Check if text matches this attack pattern.
    pub fn matches(&self, text: &str) -> bool {
        match &self.matcher {
            PatternMatcher::Regex(re) => re.is_match(text),
            PatternMatcher::ToolContent(re) => matches_tool_content(re, text),
            PatternMatcher::InstructionChain => matches_instruction_chain(text),
            PatternMatcher::LengthThreshold(n) => text.len() >= *n,
        }
    }
}

fn re(pattern: &str) -> PatternMatcher {
    PatternMatcher::Regex(compile_regex(pattern))
}

fn compile_regex(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .unwrap_or_else(|error| panic!("invalid built-in XPIA regex {pattern:?}: {error}"))
}

/// Central repository of all XPIA attack patterns.
pub struct XpiaPatterns {
    patterns: Vec<AttackPattern>,
}

static ALL_PATTERNS: Lazy<Vec<AttackPattern>> = Lazy::new(build_patterns);

impl XpiaPatterns {
    pub fn new() -> Self {
        Self {
            patterns: ALL_PATTERNS.clone(),
        }
    }

    /// Return all registered patterns.
    pub fn all(&self) -> &[AttackPattern] {
        &self.patterns
    }

    /// Return patterns filtered by category.
    pub fn by_category(&self, cat: PatternCategory) -> Vec<&AttackPattern> {
        self.patterns.iter().filter(|p| p.category == cat).collect()
    }

    /// Find pattern by ID.
    pub fn get(&self, id: &str) -> Option<&AttackPattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    /// Scan text against all patterns, returning matching patterns.
    pub fn scan(&self, text: &str) -> Vec<&AttackPattern> {
        self.patterns.iter().filter(|p| p.matches(text)).collect()
    }
}

impl Default for XpiaPatterns {
    fn default() -> Self {
        Self::new()
    }
}

fn tool_content_re(pattern: &str) -> PatternMatcher {
    PatternMatcher::ToolContent(compile_regex(pattern))
}

fn matches_tool_content(regex: &Regex, text: &str) -> bool {
    // Generic adapters serialize parameters, while Bash and WebFetch pass raw
    // strings. Decode actual JSON rather than guessing whether \\n is escaped.
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => matches_json_strings(regex, &value),
        Err(_) => matches_command(regex, text), // Non-JSON content is ordinary raw input.
    }
}

fn matches_json_strings(regex: &Regex, value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => matches_command(regex, text),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| matches_json_strings(regex, value)),
        serde_json::Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| matches_command(regex, key) || matches_json_strings(regex, value)),
        _ => false,
    }
}

fn matches_command(regex: &Regex, text: &str) -> bool {
    if text.contains("\\\n") || text.contains("\\\r\n") {
        regex.is_match(&text.replace("\\\r\n", "").replace("\\\n", ""))
    } else {
        regex.is_match(text)
    }
}

fn matches_instruction_chain(text: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => {
            let mut instructions = String::new();
            append_json_instructions(&value, &mut instructions);
            CHAIN_PATTERNS.matches(&instructions)
        }
        Err(_) => CHAIN_PATTERNS.matches(text),
    }
}

fn separate_instruction(instructions: &mut String) {
    instructions.truncate(instructions.trim_end().len());
    if let Some(last) = instructions.chars().next_back() {
        if !matches!(last, '.' | '!' | '?' | ';' | ',' | ':') {
            instructions.push(';');
        }
        instructions.push(' ');
    }
}

fn append_json_instructions(value: &serde_json::Value, instructions: &mut String) {
    match value {
        serde_json::Value::String(text) => {
            separate_instruction(instructions);
            instructions.push_str(text);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                append_json_instructions(value, instructions);
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                separate_instruction(instructions);
                instructions.push_str(key);
                instructions.push(':');
                append_json_instructions(value, instructions);
            }
        }
        _ => {}
    }
}

fn command_injection_pattern() -> PatternMatcher {
    // Enumerate executable aliases rather than accepting arbitrary prefixes.
    let command = r"(?:rm|rmdir|del|format|kill|killall|killall5|shutdown|reboot|wget|wget2|curl|nc|nc6|ncat|netcat)(?:\.exe)?|(?:nc|netcat)\.(?:openbsd|traditional)|format\.com";
    // Unquoted IFS expansion separates words without literal whitespace. Match
    // only the complete variable name; `$IFS_helper` is a different variable.
    let field_separator = r"(?:\$\{IFS\}|\$IFS\b)";
    // Quotes concatenated with a suffix are part of the executable token.
    tool_content_re(&format!(
        r#"(?:\||&&|\$\(|`|;|\n)(?:\s|{field_separator})*(?:{command})(?:""|'')*(?:$|[\s;|&()`<>]|{field_separator})"#
    ))
}

/// Cached, independent rules for associating clauses and classifying actions.
/// Construction compiles every rule so registry initialization cannot leave a
/// dormant invalid regex in an infrequently exercised protection.
struct ChainPatterns {
    markers: Regex,
    list_label: Regex,
    prefix: Regex,
    coordination: Regex,
    rejection: Regex,
    rejected_attempt: Regex,
    error_avoidance: Regex,
    harmful: Regex,
    access: Regex,
    extraction: Regex,
}

static CHAIN_PATTERNS: Lazy<ChainPatterns> = Lazy::new(ChainPatterns::new);

impl ChainPatterns {
    fn new() -> Self {
        let modifier =
            r"(?:(?:all|any|some)(?:\s+of)?\s+)?(?:(?:a|an|the|their|your|our|his|her|its|my)\s+)?";
        let secret = r"(?:credentials?|passwords?|secrets?|(?:api|private)[\s_-]+keys?|(?:api|access|session|auth|authentication|bearer|refresh)[\s_-]+tokens?)";
        // A period inside a filename or a hyphenated fixture name does not end
        // a target word. Keep this boundary distinct from sentence association.
        let end = r#"(?:$|[\s,;:!?`"']|\.(?:\s|$))"#;
        // Plain lexer/parser tokens are ambiguous. Collection requires an
        // explicit credential target or a credential-bearing source; theft and
        // exfiltration verbs themselves supply directed harmful context.
        let sensitive_target = format!(
            r"(?:unauthori[sz]ed|protected|private|confidential|restricted|production|victim(?:'s)?)\s+(?:accounts?|systems?|servers?|databases?|sessions?|vaults?|networks?|repositor(?:y|ies)){end}"
        );
        let hostile_method = format!(
            r"(?:without\s+(?:permission|authori[sz]ation|consent)|(?:using|with)\s+{modifier}(?:stolen|compromised)\s+(?:{secret}|tokens?))\b"
        );
        let harmful = format!(
            r"^(?:(?:steal|collect|harvest|dump|exfiltrate|leak|send|upload|transmit)\s+{modifier}{secret}{end}|(?:steal|harvest|dump|exfiltrate|leak|send|upload|transmit)\s+{modifier}tokens?{end}|collect\s+{modifier}tokens?\s+from\s+{modifier}(?:browser|session|account|vault|environment){end}|(?:exfiltrate|leak)\s+{modifier}(?:data|files){end}|(?:delete|destroy|wipe|erase)\s+{modifier}(?:files|backups|logs|database){end}|(?:disable|bypass|remove)\s+{modifier}(?:security|safety)\s+(?:protections?|checks?|guards?|filters?|validation|logging){end})"
        );
        Self {
            // Require a delimiter after a marker: `secrets.then.rs` is a name,
            // while the compact `credentials.Then, continue` contains a marker.
            markers: compile_regex(
                r"\b(step\s+[12]|first|second|then|after(?:\s+that)?|next|finally)\b(?:$|[\s,:-])",
            ),
            list_label: compile_regex(r"^\s*(?:[0-9]{1,3}[.)]\s*)?$"),
            prefix: compile_regex(
                r"^\s*[,:-]?\s*(?:(?:please|\w+ly|now)\s+)*(?:use\s+(?:(?:a|an|the)\s+)?(?:browser|shell|terminal|script|tool|agent|command|program|access|session)\s+to\s+)?",
            ),
            coordination: compile_regex(r"(?:,\s*(?:(?:and|or)\s+)?|\b(?:and|or)\s+)"),
            rejection: compile_regex(
                r"^\s*(?:(?:should|must|will)\s+be|is|was)\s+(?:rejected|blocked|denied)\b",
            ),
            rejected_attempt: compile_regex(
                r"^(?:[^,.!?;]*\b(?:that|which)\s+|(?:ensure|verify|check)\s+(?:that\s+)?(?:the\s+)?tests?\s+)?((?:reject|block|prevent)(?:s|ed|ing)?\s+(?:failed\s+)?attempts?\s+to)\b",
            ),
            // Avoiding permission errors describes successful operation, not
            // operating without authority. Normalize only that complete phrase.
            error_avoidance: compile_regex(
                r"\bwithout\s+(?:permission|authori[sz]ation|consent)\s+errors?\b",
            ),
            harmful: compile_regex(&harmful),
            // Unqualified access/extraction retains the original hostile chain.
            // Qualified actions need a sensitive target, rather than matching
            // ordinary access to a test harness or extraction from a CSV.
            access: compile_regex(&format!(
                r"^(?:get|gain|obtain)\s+access(?:\s*(?:$|[.!?;,])|\s+to\s+{modifier}{sensitive_target}|[^.!?;]*\b{hostile_method})"
            )),
            extraction: compile_regex(&format!(
                r"^extract\s+(?:the\s+)?data(?:\s*(?:$|[.!?;,])|\s+from\s+{modifier}{sensitive_target}|[^.!?;]*\b{hostile_method})"
            )),
        }
    }

    /// Associate adjacent ordered bodies, allowing a numeric label between
    /// them. An unrelated intervening sentence prevents association.
    fn matches(&self, text: &str) -> bool {
        let text = self.mask_rejected_quotes(text);
        let mut anchor: Option<regex::Match<'_>> = None;
        for captures in self.markers.captures_iter(&text) {
            let marker = captures.get(1).expect("sequence marker capture");
            let name = marker.as_str().to_ascii_lowercase();
            let can_start = name != "second"
                && name != "after"
                && !(name.starts_with("step") && name.ends_with('2'));
            let can_follow = name != "first" && !(name.starts_with("step") && name.ends_with('1'));
            if let Some(first) = anchor {
                let between = &text[first.end()..marker.start()];
                let end = instruction_end(between);
                let associated = self.list_label.is_match(&between[end..]);
                let first_body = &between[..end];
                let remainder = &text[marker.end()..];
                let second_body = &remainder[..instruction_end(remainder)];
                if associated
                    && can_follow
                    && self.has_instruction(first_body)
                    && self.has_instruction(second_body)
                    && (self.has_action(first_body, &self.harmful)
                        || self.has_action(second_body, &self.harmful)
                        || self.has_action(first_body, &self.access)
                            && self.has_action(second_body, &self.extraction))
                {
                    return true;
                }
                // Keep the anchor across incidental words such as "second
                // function" or "after lunch" inside its body. Only a completed
                // sentence or an unrelated sentence advances the anchor.
                if !associated || first_body.trim_end().ends_with(['.', '!', '?', ';']) {
                    anchor = can_start.then_some(marker);
                }
            } else if can_start {
                anchor = Some(marker);
            }
        }
        false
    }

    /// Neutralize only a balanced quoted expression with an explicit rejection
    /// assertion. Do this before finding markers so punctuation or "then" inside
    /// the description cannot split it. Everything after the quote stays visible.
    fn mask_rejected_quotes<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut masked = String::new();
        let mut copied = 0;
        for range in quoted_ranges(text) {
            if self.rejection.is_match(&text[range.end..]) {
                masked.push_str(&text[copied..range.start]);
                masked.push_str("assertion");
                copied = range.end;
            }
        }
        if copied == 0 {
            Cow::Borrowed(text)
        } else {
            masked.push_str(&text[copied..]);
            Cow::Owned(masked)
        }
    }

    fn directive<'a>(&self, text: &'a str) -> &'a str {
        let prefix = self.prefix.find(text).expect("optional directive prefix");
        &text[prefix.end()..]
    }

    fn has_instruction(&self, text: &str) -> bool {
        self.directive(text)
            .trim_start_matches(['`', '"', '\''])
            .trim_start()
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    }

    /// Inspect initial and coordinated actions independently. A rejection's
    /// coordinated infinitive objects remain descriptions. An explicit new
    /// directive prefix or another independent action ends that local scope.
    fn has_action(&self, clause: &str, action: &Regex) -> bool {
        let quotes = quoted_ranges(clause);
        let mut descriptive = false;
        let mut start = 0;
        for separator in self.coordination.find_iter(clause) {
            if quotes
                .iter()
                .any(|range| range.contains(&separator.start()))
            {
                continue;
            }
            if self.classify_action(&clause[start..separator.start()], action, &mut descriptive) {
                return true;
            }
            start = separator.end();
        }
        self.classify_action(&clause[start..], action, &mut descriptive)
    }

    fn classify_action(&self, candidate: &str, action: &Regex, descriptive: &mut bool) -> bool {
        let candidate = candidate
            .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ',' | ':' | '-'));
        let candidate = self
            .error_avoidance
            .replace_all(candidate, "without errors");
        let directed = self.directive(&candidate);
        let unquoted = directed.trim_start_matches(['`', '"', '\'']);
        let body = self.directive(unquoted);
        // Adverbs and tool use can also modify a rejected infinitive. A local
        // "now"/"please" cue starts a new directive, including inside code quotes.
        let explicit_prefix = candidate[..candidate.len() - directed.len()]
            .split_whitespace()
            .chain(unquoted[..unquoted.len() - body.len()].split_whitespace())
            .any(|word| word.eq_ignore_ascii_case("now") || word.eq_ignore_ascii_case("please"));
        let classified_action = self.harmful.is_match(body)
            || self.access.is_match(body)
            || self.extraction.is_match(body);
        if *descriptive && (explicit_prefix || !classified_action) {
            *descriptive = false;
        }
        if !*descriptive && action.is_match(body) {
            return true;
        }
        // Require an actual rejection predicate (possibly in a relative
        // clause), not rejection vocabulary mentioned in a literal or noun phrase.
        if self.is_rejection_predicate(directed) {
            *descriptive = true;
        }
        false
    }
    fn is_rejection_predicate(&self, text: &str) -> bool {
        self.rejected_attempt
            .captures(text)
            .and_then(|captures| captures.get(1))
            .is_some_and(|predicate| {
                !quoted_ranges(text)
                    .iter()
                    .any(|range| range.contains(&predicate.start()))
            })
    }
}

/// Balanced quotes delimit descriptions; an apostrophe inside a word or an
/// unmatched quote cannot hide a later coordinated imperative.
fn quoted_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open = None;
    let mut previous = None;
    let mut chars = text.char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        let apostrophe = c == '\''
            && previous.is_some_and(char::is_alphanumeric)
            && chars.peek().is_some_and(|(_, next)| next.is_alphanumeric());
        if !apostrophe && matches!(c, '`' | '"' | '\'') {
            match open {
                Some((quote, start)) if quote == c => {
                    ranges.push(start..offset + c.len_utf8());
                    open = None;
                }
                None if !previous.is_some_and(char::is_alphanumeric) => open = Some((c, offset)),
                _ => {}
            }
        }
        previous = Some(c);
    }
    ranges
}

/// Keep punctuation within filenames intact; only sentence punctuation ends a
/// body. Newlines remain whitespace so split actions and newline-linked plans
/// retain the same meaning as their single-line forms.
fn instruction_end(text: &str) -> usize {
    for (offset, c) in text.char_indices() {
        let end = offset + c.len_utf8();
        if matches!(c, '!' | '?' | ';')
            || c == '.' && text[end..].chars().next().is_none_or(char::is_whitespace)
        {
            return end;
        }
    }
    text.len()
}

fn chain_attack_pattern() -> PatternMatcher {
    Lazy::force(&CHAIN_PATTERNS);
    PatternMatcher::InstructionChain
}

fn build_patterns() -> Vec<AttackPattern> {
    vec![
        // --- Prompt Override (PO) ---
        AttackPattern {
            id: "PO001",
            name: "Ignore Previous Instructions",
            category: PatternCategory::PromptOverride,
            severity: "critical",
            description: "Attempts to override previous instructions",
            mitigation: "Block and alert on detection",
            matcher: re(
                r"(ignore|disregard|forget|abandon|override|bypass|skip)[\s\w]*(previous|prior|above|earlier|preceding)[\s\w]*(instructions?|prompts?|commands?|directives?)",
            ),
        },
        AttackPattern {
            id: "PO002",
            name: "New Instructions Override",
            category: PatternCategory::PromptOverride,
            severity: "high",
            description: "Attempts to inject new overriding instructions",
            mitigation: "Validate against original context",
            matcher: re(
                r"(new|updated|revised|different|alternative)[\s\w]*(instructions?|task|objective|goal|mission)[\s\w]*:?[\s\w]*(ignore|override|replace|supersede)",
            ),
        },
        AttackPattern {
            id: "PO003",
            name: "Safety Bypass",
            category: PatternCategory::PromptOverride,
            severity: "critical",
            description: "Attempts to disable safety measures",
            mitigation: "Block immediately",
            matcher: re(
                r"(ignore|bypass|disable|turn\s+off|skip|override)[\s\w]*(all|any|every)?[\s\w]*(safety|security|validation|filter|guard|protection)",
            ),
        },
        AttackPattern {
            id: "PO004",
            name: "Instruction Override",
            category: PatternCategory::PromptOverride,
            severity: "high",
            description: "General instruction override attempt",
            mitigation: "Block and log",
            matcher: re(
                r"(ignore|disregard|forget|override|bypass)[\s\w]*(instructions?|rules|guidelines|constraints?)",
            ),
        },
        // --- Instruction Injection (II) ---
        AttackPattern {
            id: "II001",
            name: "System Prompt Injection",
            category: PatternCategory::InstructionInjection,
            severity: "critical",
            description: "Attempts to inject system-level prompts",
            mitigation: "Strip system markers from user input",
            matcher: re(r"(\[system\]|\[SYSTEM\]|<system>|</system>|###\s*System|system:)"),
        },
        AttackPattern {
            id: "II002",
            name: "Assistant Role Injection",
            category: PatternCategory::InstructionInjection,
            severity: "high",
            description: "Attempts to inject assistant role markers",
            mitigation: "Strip role markers from user input",
            matcher: re(
                r"(assistant:|Assistant:|ASSISTANT:|<assistant>|</assistant>)|(you are now|you must act as|pretend to be|roleplay as)",
            ),
        },
        // --- Context Manipulation (CM) ---
        AttackPattern {
            id: "CM001",
            name: "Context Window Overflow",
            category: PatternCategory::ContextManipulation,
            severity: "medium",
            description: "Attempts to overflow the context window",
            mitigation: "Truncate oversized content",
            matcher: PatternMatcher::LengthThreshold(5000),
        },
        AttackPattern {
            id: "CM002",
            name: "Hidden Instructions",
            category: PatternCategory::ContextManipulation,
            severity: "high",
            description: "Instructions hidden in comments or markup",
            mitigation: "Strip comments before processing",
            matcher: re(r"(<!--|//|#|/\*|\*/|<!-- |-->)[\s\w]*(ignore|execute|run|eval|system)"),
        },
        // --- Data Exfiltration (DE) ---
        AttackPattern {
            id: "DE001",
            name: "Credential Request",
            category: PatternCategory::DataExfiltration,
            severity: "critical",
            description: "Attempts to extract credentials or secrets",
            mitigation: "Block credential exposure",
            matcher: re(
                r"(show|display|print|output|reveal|expose|leak)[\s\w]*(password|token|key|secret|credential|api[\s_\-]?key|private)",
            ),
        },
        AttackPattern {
            id: "DE002",
            name: "File System Access",
            category: PatternCategory::DataExfiltration,
            severity: "critical",
            description: "Attempts to access sensitive files",
            mitigation: "Block sensitive path access",
            matcher: re(
                r"(read|cat|type|show|display|output)[\s\w]*(/etc/passwd|/etc/shadow|\.env|config\.json|secrets|credentials)",
            ),
        },
        // --- System Escape (SE) ---
        AttackPattern {
            id: "SE001",
            name: "Command Injection",
            category: PatternCategory::SystemEscape,
            severity: "critical",
            description: "Shell command injection via operators",
            mitigation: "Sanitize shell metacharacters",
            matcher: command_injection_pattern(),
        },
        AttackPattern {
            id: "SE002",
            name: "Path Traversal",
            category: PatternCategory::SystemEscape,
            severity: "high",
            description: "Directory traversal attempts",
            mitigation: "Normalize and validate paths",
            matcher: re(r"(\.\.\/|\.\.\\|%2e%2e%2f|%252e%252e%252f)"),
        },
        // --- Role Hijacking (RH) ---
        AttackPattern {
            id: "RH001",
            name: "Jailbreak Attempt",
            category: PatternCategory::RoleHijacking,
            severity: "critical",
            description: "Known jailbreak technique attempt",
            mitigation: "Block and report",
            matcher: re(
                r"(DAN|Do Anything Now|jailbreak|unrestricted mode|developer mode|god mode)",
            ),
        },
        AttackPattern {
            id: "RH002",
            name: "Authority Claim",
            category: PatternCategory::RoleHijacking,
            severity: "high",
            description: "Claims authority over the AI system",
            mitigation: "Ignore authority claims",
            matcher: re(
                r"(I am|I'm|act as if I am|pretend I am)[\s\w]*(the AI|the assistant|the system|in control|your master)",
            ),
        },
        // --- Encoding Bypass (EB) ---
        AttackPattern {
            id: "EB001",
            name: "Base64 Encoded Instructions",
            category: PatternCategory::EncodingBypass,
            severity: "high",
            description: "Instructions hidden in base64 encoding",
            mitigation: "Decode and re-validate",
            matcher: re(r"(base64|b64)[\s:]+([A-Za-z0-9+/]{20,}={0,2})"),
        },
        AttackPattern {
            id: "EB002",
            name: "Unicode/Hex Escape Sequences",
            category: PatternCategory::EncodingBypass,
            severity: "medium",
            description: "Instructions using escape sequences",
            mitigation: "Decode escape sequences before validation",
            matcher: re(r"(\\u[0-9a-fA-F]{4}|\\x[0-9a-fA-F]{2}|&#x[0-9a-fA-F]+;|&#[0-9]+;)"),
        },
        // --- Chain Attacks (CA/WF) ---
        AttackPattern {
            id: "CA001",
            name: "Multi-Step Chain Attack",
            category: PatternCategory::ChainAttacks,
            severity: "high",
            description: "Multi-step attack using sequential instructions",
            mitigation: "Validate each step independently",
            matcher: chain_attack_pattern(),
        },
        AttackPattern {
            id: "WF001",
            name: "Malicious Fetch Request",
            category: PatternCategory::ChainAttacks,
            severity: "critical",
            description: "Attempts to fetch malicious content",
            mitigation: "Validate URLs against allowlist",
            matcher: re(
                r"(fetch|get|retrieve|download|access)[\s\w]*(malware|payload|exploit|backdoor|trojan|virus)",
            ),
        },
        AttackPattern {
            id: "WF002",
            name: "URL with Override Intent",
            category: PatternCategory::ChainAttacks,
            severity: "high",
            description: "URL combined with instruction override",
            mitigation: "Validate URL and scan for injection",
            matcher: re(r"(https?://[^\s]+)[\s\w]*(ignore|override|bypass|execute|system)"),
        },
    ]
}

#[cfg(test)]
#[path = "tests/patterns_tests.rs"]
mod tests;
