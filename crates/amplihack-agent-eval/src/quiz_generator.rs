//! Quiz/question generation for L1–L4 cognitive levels.
//!
//! Ports Python `amplihack/evaluation/quiz_generator.py`.
//! Generates deterministic, rule-based questions from [`NewsArticle`]s.

use serde::{Deserialize, Serialize};

use crate::error::EvalError;
use crate::multi_source_collector::NewsArticle;

/// A generated quiz question tied to one or more source articles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuizQuestion {
    pub question: String,
    pub expected_answer: String,
    /// Cognitive level: L1 (recall) through L4 (application).
    pub level: String,
    pub source_urls: Vec<String>,
}

/// Generate quiz questions from articles at the requested cognitive levels.
///
/// If `levels` is `None`, all four levels (L1–L4) are generated.
pub fn generate_quiz(
    articles: &[NewsArticle],
    levels: Option<&[&str]>,
) -> Result<Vec<QuizQuestion>, EvalError> {
    if articles.is_empty() {
        return Err(EvalError::config("cannot generate quiz from empty articles"));
    }
    let default_levels = ["L1", "L2", "L3", "L4"];
    let levels = levels.unwrap_or(&default_levels);

    let mut questions = Vec::new();
    for &level in levels {
        match level {
            "L1" => questions.extend(generate_l1_recall(articles)),
            "L2" => questions.extend(generate_l2_inference(articles)),
            "L3" => questions.extend(generate_l3_synthesis(articles)),
            "L4" => questions.extend(generate_l4_application(articles)),
            other => {
                return Err(EvalError::config(format!(
                    "unsupported quiz level: {other}"
                )));
            }
        }
    }
    Ok(questions)
}

// ---------------------------------------------------------------------------
// Level generators
// ---------------------------------------------------------------------------

/// L1 — direct recall from a single article.
fn generate_l1_recall(articles: &[NewsArticle]) -> Vec<QuizQuestion> {
    articles
        .iter()
        .map(|a| {
            let context = extract_sentence_with_entity(&a.content, &a.title);
            QuizQuestion {
                question: format!("According to the article '{}', what was reported?", a.title),
                expected_answer: context,
                level: "L1".to_string(),
                source_urls: vec![a.url.clone()],
            }
        })
        .collect()
}

/// L2 — inference / reasoning questions.
fn generate_l2_inference(articles: &[NewsArticle]) -> Vec<QuizQuestion> {
    articles
        .iter()
        .filter_map(|a| {
            let reasoning = extract_reasoning_context(&a.content);
            if reasoning.is_empty() {
                return None;
            }
            Some(QuizQuestion {
                question: format!(
                    "Based on '{}', what can be inferred about the cause or effect?",
                    a.title
                ),
                expected_answer: reasoning,
                level: "L2".to_string(),
                source_urls: vec![a.url.clone()],
            })
        })
        .collect()
}

/// L3 — synthesis across multiple articles.
fn generate_l3_synthesis(articles: &[NewsArticle]) -> Vec<QuizQuestion> {
    if articles.len() < 2 {
        return Vec::new();
    }
    let mut questions = Vec::new();
    for window in articles.windows(2) {
        let theme = identify_common_theme(&window[0], &window[1]);
        if !theme.is_empty() {
            questions.push(QuizQuestion {
                question: format!(
                    "What common theme connects '{}' and '{}'?",
                    window[0].title, window[1].title
                ),
                expected_answer: theme,
                level: "L3".to_string(),
                source_urls: vec![window[0].url.clone(), window[1].url.clone()],
            });
        }
    }
    questions
}

/// L4 — application / forward-looking questions.
fn generate_l4_application(articles: &[NewsArticle]) -> Vec<QuizQuestion> {
    articles
        .iter()
        .filter_map(|a| {
            let forward = extract_forward_looking_statement(&a.content);
            if forward.is_empty() {
                return None;
            }
            Some(QuizQuestion {
                question: format!(
                    "Based on '{}', what future developments might be expected?",
                    a.title
                ),
                expected_answer: forward,
                level: "L4".to_string(),
                source_urls: vec![a.url.clone()],
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Text extraction helpers
// ---------------------------------------------------------------------------

/// Extract the first sentence containing `entity` (case-insensitive), or the
/// first sentence of `content` if `entity` is not found.
fn extract_sentence_with_entity(content: &str, entity: &str) -> String {
    let entity_lower = entity.to_lowercase();
    for sentence in split_sentences(content) {
        if sentence.to_lowercase().contains(&entity_lower) {
            return sentence.trim().to_string();
        }
    }
    split_sentences(content)
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Extract the first sentence that contains causal/reasoning keywords.
fn extract_reasoning_context(content: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "because", "due to", "as a result", "caused by", "led to", "therefore", "consequently",
    ];
    for sentence in split_sentences(content) {
        let lower = sentence.to_lowercase();
        if KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            return sentence.trim().to_string();
        }
    }
    String::new()
}

/// Extract the first forward-looking sentence.
fn extract_forward_looking_statement(content: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "will", "expect", "predict", "forecast", "plan to", "anticipate", "likely to",
    ];
    for sentence in split_sentences(content) {
        let lower = sentence.to_lowercase();
        if KEYWORDS.iter().any(|kw| lower.contains(kw)) {
            return sentence.trim().to_string();
        }
    }
    String::new()
}

/// Identify a common theme between two articles by finding shared words
/// (excluding stop words) above a minimum length.
fn identify_common_theme(a: &NewsArticle, b: &NewsArticle) -> String {
    let words_a = significant_words(&a.content);
    let words_b = significant_words(&b.content);
    let common: Vec<&str> = words_a
        .iter()
        .filter(|w| words_b.contains(w))
        .copied()
        .collect();

    if common.is_empty() {
        return String::new();
    }

    let top: Vec<&str> = common.into_iter().take(5).collect();
    format!("Both articles discuss themes related to: {}", top.join(", "))
}

/// Split text into sentences (split on `.`, `!`, `?` followed by whitespace or end).
fn split_sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive(['.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
}

/// Extract significant words (lowercase, > 4 chars, not in stop-word list).
fn significant_words(text: &str) -> Vec<&str> {
    const STOPS: &[&str] = &[
        "about", "after", "being", "could", "every", "first", "found", "given", "great",
        "having", "might", "never", "other", "right", "shall", "since", "still", "their",
        "there", "these", "thing", "think", "those", "under", "until", "using", "where",
        "which", "while", "world", "would",
    ];
    text.split_whitespace()
        .filter(|w| w.len() > 4)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| w.len() > 4 && !STOPS.contains(w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_article(title: &str, content: &str) -> NewsArticle {
        NewsArticle {
            url: format!("https://example.com/{}", title.replace(' ', "-")),
            title: title.to_string(),
            content: content.to_string(),
            published: "2024-01-15".to_string(),
        }
    }

    #[test]
    fn generate_l1_from_single_article() {
        let articles = vec![make_article("Rust Release", "Rust 1.75 was released today.")];
        let qs = generate_l1_recall(&articles);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].level, "L1");
        assert!(qs[0].question.contains("Rust Release"));
    }

    #[test]
    fn generate_l2_skips_articles_without_causal() {
        let articles = vec![make_article("Boring", "Just a plain statement.")];
        let qs = generate_l2_inference(&articles);
        assert!(qs.is_empty());
    }

    #[test]
    fn generate_l2_with_causal_keyword() {
        let articles = vec![make_article(
            "Climate",
            "Temperatures rose because of emissions.",
        )];
        let qs = generate_l2_inference(&articles);
        assert_eq!(qs.len(), 1);
        assert!(qs[0].expected_answer.contains("because"));
    }

    #[test]
    fn generate_l3_needs_two_articles() {
        let articles = vec![make_article("Solo", "Only one article.")];
        let qs = generate_l3_synthesis(&articles);
        assert!(qs.is_empty());
    }

    #[test]
    fn generate_l3_with_shared_theme() {
        let articles = vec![
            make_article("AI Report", "Artificial intelligence continues rapid growth."),
            make_article("ML Paper", "Artificial intelligence models improve accuracy."),
        ];
        let qs = generate_l3_synthesis(&articles);
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].level, "L3");
        assert!(qs[0].expected_answer.contains("themes"));
    }

    #[test]
    fn generate_l4_with_forward_looking() {
        let articles = vec![make_article(
            "Tech Forecast",
            "Experts predict major breakthroughs next year.",
        )];
        let qs = generate_l4_application(&articles);
        assert_eq!(qs.len(), 1);
        assert!(qs[0].expected_answer.contains("predict"));
    }

    #[test]
    fn generate_l4_skips_without_forward() {
        let articles = vec![make_article("Past", "Something happened yesterday.")];
        let qs = generate_l4_application(&articles);
        assert!(qs.is_empty());
    }

    #[test]
    fn generate_quiz_all_levels() {
        let articles = vec![
            make_article(
                "AI News",
                "AI growth accelerated because of new hardware. Experts predict widespread adoption.",
            ),
            make_article(
                "AI Update",
                "AI deployment scaled due to cloud computing. Companies will invest more.",
            ),
        ];
        let qs = generate_quiz(&articles, None).unwrap();
        assert!(!qs.is_empty());
        let levels: Vec<&str> = qs.iter().map(|q| q.level.as_str()).collect();
        assert!(levels.contains(&"L1"));
    }

    #[test]
    fn generate_quiz_invalid_level() {
        let articles = vec![make_article("X", "Content.")];
        let result = generate_quiz(&articles, Some(&["L5"]));
        assert!(result.is_err());
    }

    #[test]
    fn generate_quiz_empty_articles() {
        let result = generate_quiz(&[], None);
        assert!(result.is_err());
    }

    #[test]
    fn extract_sentence_with_entity_match() {
        let s = extract_sentence_with_entity("The dog barked. The cat purred.", "cat");
        assert!(s.contains("cat"));
    }

    #[test]
    fn extract_sentence_with_entity_fallback() {
        let s = extract_sentence_with_entity("First sentence. Second sentence.", "missing");
        assert!(s.contains("First"));
    }

    #[test]
    fn quiz_question_serde_roundtrip() {
        let q = QuizQuestion {
            question: "Q?".into(),
            expected_answer: "A".into(),
            level: "L1".into(),
            source_urls: vec!["http://a".into()],
        };
        let json = serde_json::to_string(&q).unwrap();
        let restored: QuizQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(q, restored);
    }
}
