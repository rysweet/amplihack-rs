//! Scan unified diffs for invisible or misleading Unicode characters.

use std::io::{self, Read};

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    file: String,
    line: usize,
    col: usize,
    codepoint: u32,
    name: String,
}

fn classify_char(ch: char) -> Option<String> {
    let codepoint = ch as u32;
    let name = match codepoint {
        0x200B => "ZERO WIDTH SPACE",
        0x200C => "ZERO WIDTH NON-JOINER",
        0x200D => "ZERO WIDTH JOINER",
        0x200E => "LEFT-TO-RIGHT MARK",
        0x200F => "RIGHT-TO-LEFT MARK",
        0x202A => "LEFT-TO-RIGHT EMBEDDING",
        0x202B => "RIGHT-TO-LEFT EMBEDDING",
        0x202C => "POP DIRECTIONAL FORMATTING",
        0x202D => "LEFT-TO-RIGHT OVERRIDE",
        0x202E => "RIGHT-TO-LEFT OVERRIDE",
        0x2066 => "LEFT-TO-RIGHT ISOLATE",
        0x2067 => "RIGHT-TO-LEFT ISOLATE",
        0x2068 => "FIRST STRONG ISOLATE",
        0x2069 => "POP DIRECTIONAL ISOLATE",
        0x2060 => "WORD JOINER",
        0x2061 => "FUNCTION APPLICATION",
        0x2062 => "INVISIBLE TIMES",
        0x2063 => "INVISIBLE SEPARATOR",
        0x2064 => "INVISIBLE PLUS",
        0xFEFF => "BYTE ORDER MARK",
        0x00AD => "SOFT HYPHEN",
        0x034F => "COMBINING GRAPHEME JOINER",
        0x061C => "ARABIC LETTER MARK",
        0x180E => "MONGOLIAN VOWEL SEPARATOR",
        0xFFF9 => "INTERLINEAR ANNOTATION ANCHOR",
        0xFFFA => "INTERLINEAR ANNOTATION SEPARATOR",
        0xFFFB => "INTERLINEAR ANNOTATION TERMINATOR",
        0xE0001 => "LANGUAGE TAG",
        0xE0020..=0xE007F => return Some(format!("TAG CHARACTER U+{codepoint:04X}")),
        0xFE00..=0xFE0F => return Some(format!("VARIATION SELECTOR U+{codepoint:04X}")),
        0xE0100..=0xE01EF => {
            return Some(format!("VARIATION SELECTOR SUPPLEMENT U+{codepoint:04X}"));
        }
        _ => return None,
    };
    Some(name.to_string())
}

fn parse_new_line_start(hunk_header: &str) -> usize {
    hunk_header
        .split('+')
        .nth(1)
        .and_then(|rest| rest.split("@@").next())
        .map(str::trim)
        .and_then(|part| part.split(',').next())
        .and_then(|line| line.parse::<usize>().ok())
        .unwrap_or(1)
}

fn scan_diff(diff_text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut current_file = "<unknown>".to_string();
    let mut line_in_new = 0usize;

    for raw_line in diff_text.lines() {
        if let Some(file) = raw_line.strip_prefix("+++ b/") {
            current_file = file.to_string();
            continue;
        }
        if raw_line.starts_with("--- ") {
            continue;
        }
        if raw_line.starts_with("@@") {
            line_in_new = parse_new_line_start(raw_line).saturating_sub(1);
            continue;
        }
        if raw_line.starts_with('+') && !raw_line.starts_with("+++") {
            line_in_new += 1;
            for (col, ch) in raw_line[1..].chars().enumerate() {
                if let Some(name) = classify_char(ch) {
                    findings.push(Finding {
                        file: current_file.clone(),
                        line: line_in_new,
                        col: col + 1,
                        codepoint: ch as u32,
                        name,
                    });
                }
            }
        } else if !raw_line.starts_with('-') {
            line_in_new += 1;
        }
    }

    findings
}

fn run() -> Result<i32, String> {
    let mut diff_text = String::new();
    io::stdin()
        .read_to_string(&mut diff_text)
        .map_err(|e| format!("failed to read input: {e}"))?;

    if diff_text.trim().is_empty() {
        println!("No diff content to scan.");
        return Ok(0);
    }

    let findings = scan_diff(&diff_text);
    if findings.is_empty() {
        println!("No invisible/malicious Unicode characters found.");
        return Ok(0);
    }

    println!(
        "Found {} suspicious invisible character(s):\n",
        findings.len()
    );
    for finding in findings {
        println!(
            "  {}:{}:{}  U+{:04X} ({})",
            finding.file, finding.line, finding.col, finding.codepoint, finding.name
        );
    }
    println!("\nThese characters are invisible and can be used to hide malicious code.");
    println!("See: https://trojansource.codes/");
    Ok(1)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_diff_has_no_findings() {
        let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,1 @@
-old
+new
";
        assert!(scan_diff(diff).is_empty());
    }

    #[test]
    fn flags_invisible_chars_on_added_lines_only() {
        let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,2 +10,2 @@
-removed\u{200B}
+added\u{200B}
 context
";
        let findings = scan_diff(diff);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].file, "src/lib.rs");
        assert_eq!(findings[0].line, 10);
        assert_eq!(findings[0].col, 6);
        assert_eq!(findings[0].codepoint, 0x200B);
        assert_eq!(findings[0].name, "ZERO WIDTH SPACE");
    }

    #[test]
    fn flags_tag_range() {
        let diff = "\
diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,1 +1,1 @@
+x\u{E0020}
";
        let findings = scan_diff(diff);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].name, "TAG CHARACTER U+E0020");
    }
}
