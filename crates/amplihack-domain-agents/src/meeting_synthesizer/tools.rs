//! Meeting synthesizer domain tools.
//!
//! Pure functions for extracting information from meeting transcripts.
//! Ports `domain_agents/meeting_synthesizer/tools.py`.

use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Extract action items from a meeting transcript.
///
/// Looks for assignment patterns like:
/// - "X, can you/please do Y by Z"
/// - "I will/I'll do Y"
/// - "X will do Y"
pub fn extract_action_items(transcript: &str) -> Vec<Value> {
    if transcript.trim().is_empty() {
        return vec![];
    }

    let mut items = Vec::new();

    let assignment_patterns =
        [r"(?i)(\w+),?\s+(?:can you|please|could you)\s+(.+?)(?:\s+by\s+(.+?))?[.?]?\s*$"];
    let assignment_res: Vec<Regex> = assignment_patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();
    let self_assign_re =
        Regex::new(r"(?i)I(?:\s+will|'ll)\s+(.+?)(?:\s+by\s+(.+?))?[.?]?\s*$").ok();
    let third_person_re = Regex::new(r"(?i)(\w+)\s+will\s+(.+?)(?:\s+by\s+(.+?))?[.?]?\s*$").ok();

    let false_positives: HashSet<&str> = ["it", "this", "that", "which", "we", "they", "the"]
        .iter()
        .copied()
        .collect();

    for line in transcript.lines() {
        let (speaker, content) = split_speaker_content(line);

        let mut matched = false;
        for re in &assignment_res {
            if let Some(cap) = re.captures(content) {
                let owner = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let action = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                let deadline = cap.get(3).map(|m| m.as_str().trim()).unwrap_or("");
                items.push(serde_json::json!({
                    "owner": owner, "action": action,
                    "deadline": deadline, "source_line": line.trim(),
                }));
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }

        // Self-assignment: "I will..."
        if let Some(ref re) = self_assign_re
            && let Some(cap) = re.captures(content)
            && !speaker.is_empty()
        {
            let action = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let deadline = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            items.push(serde_json::json!({
                "owner": speaker, "action": action,
                "deadline": deadline, "source_line": line.trim(),
            }));
            continue;
        }

        // Third-person: "Bob will..."
        if let Some(ref re) = third_person_re
            && let Some(cap) = re.captures(content)
        {
            let owner = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if !false_positives.contains(owner.to_lowercase().as_str()) {
                let action = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                let deadline = cap.get(3).map(|m| m.as_str().trim()).unwrap_or("");
                items.push(serde_json::json!({
                    "owner": owner, "action": action,
                    "deadline": deadline, "source_line": line.trim(),
                }));
            }
        }
    }

    items
}

/// Generate a summary of a meeting transcript.
pub fn generate_summary(transcript: &str) -> Value {
    if transcript.trim().is_empty() {
        return serde_json::json!({
            "summary_text": "", "participants": [], "word_count": 0,
            "line_count": 0, "duration_estimate": "", "key_points": [],
        });
    }

    let lines: Vec<&str> = transcript.lines().collect();
    let word_count = transcript.split_whitespace().count();
    let participants = extract_speakers(&lines);
    let estimated_minutes = (word_count / 150).max(1);
    let duration_estimate = if estimated_minutes < 5 {
        format!("~{estimated_minutes} minute(s)")
    } else {
        format!("~{estimated_minutes} minutes")
    };

    let key_indicators = [
        "think",
        "should",
        "need",
        "prioritize",
        "agree",
        "decided",
        "proposal",
        "important",
        "plan",
        "finish",
        "complete",
    ];
    let mut key_points = Vec::new();
    for line in &lines {
        let content = if let Some(idx) = line.find(':') {
            line[idx + 1..].trim()
        } else {
            line.trim()
        };
        if content.is_empty() {
            continue;
        }
        let lower = content.to_lowercase();
        if key_indicators.iter().any(|ind| lower.contains(ind)) {
            let truncated: String = content.chars().take(200).collect();
            key_points.push(truncated);
        }
    }
    key_points.truncate(10);

    let mut summary_parts = Vec::new();
    if !participants.is_empty() {
        summary_parts.push(format!(
            "Meeting with {} participants: {}.",
            participants.len(),
            participants.join(", ")
        ));
    }
    summary_parts.push(format!("Estimated duration: {duration_estimate}."));
    if !key_points.is_empty() {
        summary_parts.push(format!("Key points discussed: {}.", key_points.len()));
    }

    serde_json::json!({
        "summary_text": summary_parts.join(" "),
        "participants": participants,
        "word_count": word_count,
        "line_count": lines.len(),
        "duration_estimate": duration_estimate,
        "key_points": key_points,
    })
}

/// Identify decisions made during a meeting.
pub fn identify_decisions(transcript: &str) -> Vec<Value> {
    if transcript.trim().is_empty() {
        return vec![];
    }

    let patterns = [
        r"(?i)\b(?:decided|agreed|approved|resolved)\b",
        r"(?i)\bwill\s+go\s+with\b",
        r"(?i)\b(?:consensus|chosen|selected|concluded)\b",
        r"(?i)\b(?:let'?s\s+go\s+with|we'?ll\s+use|moving\s+forward\s+with)\b",
    ];

    let compiled: Vec<Regex> = patterns.iter().filter_map(|p| Regex::new(p).ok()).collect();

    let mut decisions = Vec::new();
    for line in transcript.lines() {
        let (speaker, content) = split_speaker_content(line);
        for re in &compiled {
            if re.is_match(content) {
                let truncated: String = content.chars().take(300).collect();
                decisions.push(serde_json::json!({
                    "decision": truncated,
                    "speaker": speaker,
                    "context": line.trim(),
                }));
                break;
            }
        }
    }

    decisions
}

/// Identify topics discussed in a meeting.
pub fn identify_topics(transcript: &str) -> Vec<Value> {
    if transcript.trim().is_empty() {
        return vec![];
    }

    let lines: Vec<&str> = transcript.lines().collect();
    let topic_patterns = [
        r"(?i)\blet'?s\s+(?:discuss|talk\s+about|move\s+to|look\s+at)\b",
        r"(?i)\b(?:next|moving\s+on|regarding|about|re:)\b",
        r"(?i)\b(?:agenda\s+item|topic|subject)\b",
    ];

    let topic_res: Vec<Regex> = topic_patterns
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect();

    let mut topics = Vec::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut current_speakers: HashSet<String> = HashSet::new();
    let mut start_line = 0;

    for (i, line) in lines.iter().enumerate() {
        let (speaker, content) = split_speaker_content(line);
        if !speaker.is_empty() && speaker.len() < 30 {
            current_speakers.insert(speaker.to_string());
        }

        let is_new_topic = topic_res.iter().any(|r| r.is_match(content));

        if is_new_topic
            && !current_lines.is_empty()
            && let Some(topic_text) = infer_topic(&current_lines)
        {
            topics.push(serde_json::json!({
                "topic": topic_text,
                "speakers": current_speakers.iter().collect::<Vec<_>>(),
                "line_range": [start_line + 1, i],
            }));
            current_lines.clear();
            current_speakers.clear();
            if !speaker.is_empty() {
                current_speakers.insert(speaker.to_string());
            }
            start_line = i;
        } else if is_new_topic && !current_lines.is_empty() {
            current_lines.clear();
            current_speakers.clear();
            if !speaker.is_empty() {
                current_speakers.insert(speaker.to_string());
            }
            start_line = i;
        }

        current_lines.push(content.to_string());
    }

    // Save last topic
    if !current_lines.is_empty()
        && let Some(topic_text) = infer_topic(&current_lines)
    {
        topics.push(serde_json::json!({
            "topic": topic_text,
            "speakers": current_speakers.iter().collect::<Vec<_>>(),
            "line_range": [start_line + 1, lines.len()],
        }));
    }

    // Fallback: single topic from all content
    if topics.is_empty() {
        let all_speakers = extract_speakers(&lines);
        let content_lines: Vec<String> = lines
            .iter()
            .map(|l| {
                if let Some(idx) = l.find(':') {
                    l[idx + 1..].trim().to_string()
                } else {
                    l.trim().to_string()
                }
            })
            .collect();
        if let Some(topic_text) = infer_topic(&content_lines) {
            topics.push(serde_json::json!({
                "topic": topic_text, "speakers": all_speakers,
                "line_range": [1, lines.len()],
            }));
        }
    }

    topics
}

fn split_speaker_content(line: &str) -> (&str, &str) {
    if let Some(idx) = line.find(':') {
        let speaker = line[..idx].trim();
        let content = line[idx + 1..].trim();
        if speaker.len() < 30 {
            (speaker, content)
        } else {
            ("", line.trim())
        }
    } else {
        ("", line.trim())
    }
}

fn extract_speakers(lines: &[&str]) -> Vec<String> {
    let mut speakers = Vec::new();
    let mut seen = HashSet::new();
    for line in lines {
        if let Some(idx) = line.find(':') {
            let speaker = line[..idx].trim();
            if !speaker.is_empty() && speaker.len() < 30 {
                let key = speaker.to_lowercase();
                if seen.insert(key) {
                    speakers.push(speaker.to_string());
                }
            }
        }
    }
    speakers
}

fn infer_topic(lines: &[String]) -> Option<String> {
    let prefixes = [
        "let's discuss ",
        "let's talk about ",
        "moving on to ",
        "regarding ",
    ];
    for line in lines {
        let clean = line.trim();
        if clean.len() > 10 {
            let mut topic: String = clean.chars().take(100).collect();
            let lower = topic.to_lowercase();
            for prefix in &prefixes {
                if lower.starts_with(prefix) {
                    topic = topic[prefix.len()..].to_string();
                    break;
                }
            }
            return Some(topic.trim().trim_end_matches('.').to_string());
        }
    }
    None
}

/// Helper to extract a string field from a task map.
pub(crate) fn get_str<'a>(task: &'a HashMap<String, serde_json::Value>, key: &str) -> &'a str {
    task.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_action_items_assignment() {
        let transcript = "Alice: Bob, can you draft the API spec by Friday?\n";
        let items = extract_action_items(transcript);
        assert!(!items.is_empty());
        assert_eq!(items[0]["owner"], "Bob");
    }

    #[test]
    fn extract_action_items_self_assignment() {
        let transcript = "Charlie: I'll update the test suite by end of week.\n";
        let items = extract_action_items(transcript);
        assert!(!items.is_empty());
        assert_eq!(items[0]["owner"], "Charlie");
    }

    #[test]
    fn extract_action_items_empty() {
        let items = extract_action_items("");
        assert!(items.is_empty());
    }

    #[test]
    fn generate_summary_basic() {
        let transcript = "Alice: Hello.\nBob: Let's discuss the plan.\n";
        let summary = generate_summary(transcript);
        let participants = summary["participants"].as_array().unwrap();
        assert_eq!(participants.len(), 2);
    }

    #[test]
    fn identify_decisions_basic() {
        let transcript = "Charlie: We decided to use PostgreSQL.\n";
        let decisions = identify_decisions(transcript);
        assert!(!decisions.is_empty());
        assert_eq!(decisions[0]["speaker"], "Charlie");
    }

    #[test]
    fn identify_topics_basic() {
        let transcript =
            "Alice: Let's discuss the roadmap.\nBob: I think we should focus on APIs.\n";
        let topics = identify_topics(transcript);
        assert!(!topics.is_empty());
    }

    #[test]
    fn extract_speakers_unique() {
        let lines = vec!["Alice: Hi", "Bob: Hello", "Alice: Again"];
        let speakers = extract_speakers(&lines);
        assert_eq!(speakers.len(), 2);
    }
}
