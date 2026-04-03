use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainAgentType {
    Teaching,
    Security,
    CodeSynthesis,
    CodeReview,
    MeetingSynthesizer,
    Learning,
    Research,
}

impl std::fmt::Display for DomainAgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Teaching => write!(f, "teaching"),
            Self::Security => write!(f, "security"),
            Self::CodeSynthesis => write!(f, "code_synthesis"),
            Self::CodeReview => write!(f, "code_review"),
            Self::MeetingSynthesizer => write!(f, "meeting_synthesizer"),
            Self::Learning => write!(f, "learning"),
            Self::Research => write!(f, "research"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeachingConfig {
    pub max_quiz_questions: usize,
    pub difficulty_level: String,
    pub subject_area: String,
}

impl Default for TeachingConfig {
    fn default() -> Self {
        Self {
            max_quiz_questions: 10,
            difficulty_level: "medium".into(),
            subject_area: "general".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub scan_depth: u32,
    pub severity_threshold: String,
    pub include_info: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            scan_depth: 3,
            severity_threshold: "medium".into(),
            include_info: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSynthesisConfig {
    pub language: String,
    pub style: String,
    pub max_complexity: u32,
}

impl Default for CodeSynthesisConfig {
    fn default() -> Self {
        Self {
            language: "rust".into(),
            style: "idiomatic".into(),
            max_complexity: 10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearningConfig {
    pub retention_strategy: String,
    pub max_memory_items: usize,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            retention_strategy: "spaced_repetition".into(),
            max_memory_items: 1000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub agent_type: DomainAgentType,
    pub confidence: f64,
    pub reasoning: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeachingResult {
    pub content_delivered: String,
    pub topics_covered: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuizQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub correct_index: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationResult {
    pub score: f64,
    pub feedback: String,
    pub correct_count: usize,
    pub total_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vulnerability {
    pub id: String,
    pub severity: String,
    pub description: String,
    pub location: String,
    pub recommendation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuditReport {
    pub vulnerabilities: Vec<Vulnerability>,
    pub risk_score: f64,
    pub summary: String,
    pub scanned_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub overall_risk: String,
    pub risk_score: f64,
    pub factors: Vec<String>,
    pub recommendations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeSpec {
    pub description: String,
    pub language: String,
    pub constraints: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedCode {
    pub code: String,
    pub language: String,
    pub explanation: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodeAnalysis {
    pub complexity: u32,
    pub issues: Vec<String>,
    pub suggestions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearnedContent {
    pub content_id: String,
    pub summary: String,
    pub key_concepts: Vec<String>,
    pub learned_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub content: String,
    pub confidence: f64,
    pub sources: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_agent_type_display() {
        assert_eq!(DomainAgentType::Teaching.to_string(), "teaching");
        assert_eq!(DomainAgentType::Security.to_string(), "security");
        assert_eq!(DomainAgentType::CodeSynthesis.to_string(), "code_synthesis");
        assert_eq!(DomainAgentType::CodeReview.to_string(), "code_review");
        assert_eq!(DomainAgentType::MeetingSynthesizer.to_string(), "meeting_synthesizer");
        assert_eq!(DomainAgentType::Learning.to_string(), "learning");
        assert_eq!(DomainAgentType::Research.to_string(), "research");
    }

    #[test]
    fn domain_agent_type_serde_roundtrip() {
        let variants = [
            DomainAgentType::Teaching,
            DomainAgentType::Security,
            DomainAgentType::CodeSynthesis,
            DomainAgentType::CodeReview,
            DomainAgentType::MeetingSynthesizer,
            DomainAgentType::Learning,
            DomainAgentType::Research,
        ];
        for variant in &variants {
            let json = serde_json::to_string(variant).unwrap();
            let back: DomainAgentType = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, variant);
        }
    }

    #[test]
    fn domain_agent_type_rename_all() {
        let json = serde_json::to_string(&DomainAgentType::CodeSynthesis).unwrap();
        assert_eq!(json, r#""code_synthesis""#);
    }

    #[test]
    fn teaching_config_default() {
        let cfg = TeachingConfig::default();
        assert_eq!(cfg.max_quiz_questions, 10);
        assert_eq!(cfg.difficulty_level, "medium");
        assert_eq!(cfg.subject_area, "general");
    }

    #[test]
    fn teaching_config_serde_roundtrip() {
        let cfg = TeachingConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TeachingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn security_config_default() {
        let cfg = SecurityConfig::default();
        assert_eq!(cfg.scan_depth, 3);
        assert_eq!(cfg.severity_threshold, "medium");
        assert!(!cfg.include_info);
    }

    #[test]
    fn security_config_serde_roundtrip() {
        let cfg = SecurityConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SecurityConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn code_synthesis_config_default() {
        let cfg = CodeSynthesisConfig::default();
        assert_eq!(cfg.language, "rust");
        assert_eq!(cfg.style, "idiomatic");
        assert_eq!(cfg.max_complexity, 10);
    }

    #[test]
    fn learning_config_default() {
        let cfg = LearningConfig::default();
        assert_eq!(cfg.retention_strategy, "spaced_repetition");
        assert_eq!(cfg.max_memory_items, 1000);
    }

    #[test]
    fn routing_decision_serde_roundtrip() {
        let decision = RoutingDecision {
            agent_type: DomainAgentType::Teaching,
            confidence: 0.95,
            reasoning: "educational query".into(),
        };
        let json = serde_json::to_string(&decision).unwrap();
        let back: RoutingDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(back, decision);
    }

    #[test]
    fn quiz_question_serde_roundtrip() {
        let q = QuizQuestion {
            question: "What is Rust?".into(),
            options: vec!["A language".into(), "A fruit".into()],
            correct_index: 0,
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: QuizQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(back, q);
    }

    #[test]
    fn vulnerability_serde_roundtrip() {
        let vuln = Vulnerability {
            id: "CVE-2024-001".into(),
            severity: "high".into(),
            description: "buffer overflow".into(),
            location: "src/main.rs:42".into(),
            recommendation: "use safe indexing".into(),
        };
        let json = serde_json::to_string(&vuln).unwrap();
        let back: Vulnerability = serde_json::from_str(&json).unwrap();
        assert_eq!(back, vuln);
    }

    #[test]
    fn audit_report_serde_roundtrip() {
        let report = AuditReport {
            vulnerabilities: vec![],
            risk_score: 2.5,
            summary: "clean".into(),
            scanned_at: Utc::now(),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: AuditReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn learned_content_serde_roundtrip() {
        let content = LearnedContent {
            content_id: "lc-1".into(),
            summary: "ownership in Rust".into(),
            key_concepts: vec!["borrow".into(), "move".into()],
            learned_at: Utc::now(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let back: LearnedContent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, content);
    }

    #[test]
    fn answer_serde_roundtrip() {
        let answer = Answer {
            content: "Rust ensures memory safety".into(),
            confidence: 0.9,
            sources: vec!["docs".into()],
        };
        let json = serde_json::to_string(&answer).unwrap();
        let back: Answer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, answer);
    }
}
