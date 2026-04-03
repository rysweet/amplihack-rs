//! Code Review domain agent.
//!
//! Ports `domain_agents/code_review/agent.py`: CodeReviewAgent that
//! reviews code for quality, security, and performance.

pub mod eval_levels;
pub mod tools;

use std::collections::HashMap;

use crate::base::{DomainAgent, DomainTeachingResult, EvalLevel, TaskResult};
use crate::error::Result;

use tools::{analyze_code, check_style, detect_security_issues, suggest_improvements};

const DEFAULT_PROMPT: &str = "You are an expert code reviewer.";

/// Agent that reviews code for quality, security, and performance.
pub struct CodeReviewAgent {
    agent_name: String,
    model: String,
}

impl CodeReviewAgent {
    pub fn new(agent_name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            model: model.into(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new("code_review_agent", "gpt-4o-mini")
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl DomainAgent for CodeReviewAgent {
    fn domain(&self) -> &str {
        "code_review"
    }

    fn agent_name(&self) -> &str {
        &self.agent_name
    }

    fn system_prompt(&self) -> String {
        DEFAULT_PROMPT.to_string()
    }

    fn execute_task(&self, task: &HashMap<String, serde_json::Value>) -> Result<TaskResult> {
        let code = tools::get_str(task, "code");
        let language = task
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("python");

        if code.trim().is_empty() {
            return Ok(TaskResult::fail("No code provided for review"));
        }

        let analysis = analyze_code(code, language);
        let style_issues = check_style(code, language);
        let security_issues = detect_security_issues(code, language);
        let improvement_suggestions = suggest_improvements(code, language);

        let mut all_issues: Vec<serde_json::Value> = Vec::new();
        all_issues.extend(style_issues.iter().cloned());
        all_issues.extend(security_issues.iter().cloned());
        all_issues.extend(improvement_suggestions.iter().cloned());

        let critical = all_issues
            .iter()
            .filter(|i| i.get("severity").and_then(|v| v.as_str()) == Some("critical"))
            .count();
        let high = all_issues
            .iter()
            .filter(|i| i.get("severity").and_then(|v| v.as_str()) == Some("high"))
            .count();
        let warning = all_issues
            .iter()
            .filter(|i| i.get("severity").and_then(|v| v.as_str()) == Some("warning"))
            .count();

        let score = (1.0 - critical as f64 * 0.2 - high as f64 * 0.1 - warning as f64 * 0.05)
            .clamp(0.0, 1.0);

        let line_count = analysis
            .get("line_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let summary = format!(
            "Score: {:.0}% | Issues: {} | Lines: {}",
            score * 100.0,
            all_issues.len(),
            line_count
        );

        let output = serde_json::json!({
            "issues": all_issues,
            "issue_count": all_issues.len(),
            "score": (score * 100.0).round() / 100.0,
            "summary": summary,
            "tool_results": {
                "analyze_code": analysis,
                "check_style": style_issues,
                "detect_security_issues": security_issues,
                "suggest_improvements": improvement_suggestions,
            },
        });

        let mut meta = HashMap::new();
        meta.insert("language".into(), serde_json::json!(language));
        Ok(TaskResult::ok_with_meta(output, meta))
    }

    fn eval_levels(&self) -> Vec<EvalLevel> {
        eval_levels::get_eval_levels()
    }

    fn teach(&self, topic: &str, student_level: &str) -> Result<DomainTeachingResult> {
        let key = topic
            .split_whitespace()
            .next()
            .unwrap_or("quality")
            .to_lowercase();

        let lesson_plan = match key.as_str() {
            "security" => {
                "1. Common vulnerabilities\n2. SQL injection\n3. Secrets management\n4. Input validation\n5. Practice review"
            }
            "style" => {
                "1. Why style matters\n2. PEP 8 naming\n3. Documentation\n4. Code organization\n5. Practice"
            }
            _ => "1. What is quality?\n2. Bug patterns\n3. Error handling\n4. Testing\n5. Practice",
        };
        let mut plan = lesson_plan.to_string();
        if student_level == "advanced" {
            plan.push_str("\n6. Advanced: Design patterns");
        }

        let instruction = match key.as_str() {
            "security" => {
                "When reviewing for security:\n\n1. **SQL Injection**: Never use f-strings for SQL.\n2. **Hardcoded Secrets**: Use environment variables.\n3. **Dangerous Functions**: eval(), exec() with untrusted input.\n4. **Command Injection**: Use subprocess with shell=False."
            }
            "style" => {
                "Python style review:\n\n1. **Naming**: snake_case for functions, PascalCase for classes\n2. **Docstrings**: Every public function needs one\n3. **Line Length**: Under 120 characters\n4. **Exception Handling**: Never use bare except clauses"
            }
            _ => {
                "Code quality review:\n\n1. **Bug Detection**: undefined variables, off-by-one, None handling\n2. **Error Handling**: graceful, not swallowed\n3. **Edge Cases**: empty inputs, boundaries\n4. **Testing**: adequate coverage"
            }
        };

        let practice_code = match key.as_str() {
            "security" => {
                "def auth(cursor, user, pw):\n    cursor.execute(f\"SELECT * FROM users WHERE user='{user}' AND pw='{pw}'\")\n    return cursor.fetchone()\n"
            }
            "style" => {
                "class dataProcessor:\n    def processData(self, inputData):\n        try:\n            return [x*2 for x in inputData]\n        except:\n            pass\n"
            }
            _ => {
                "def get_item(items, idx):\n    return items[idx]\ndef average(nums):\n    return sum(nums) / len(nums)\n"
            }
        };

        let issues = if key.contains("secur") {
            detect_security_issues(practice_code, "python")
        } else if key.contains("style") {
            check_style(practice_code, "python")
        } else {
            suggest_improvements(practice_code, "python")
        };

        let attempt = if !issues.is_empty() {
            let findings: Vec<String> = issues
                .iter()
                .take(5)
                .map(|i| {
                    format!(
                        "- {}: {}",
                        i.get("type").and_then(|v| v.as_str()).unwrap_or("issue"),
                        i.get("message").and_then(|v| v.as_str()).unwrap_or("")
                    )
                })
                .collect();
            format!("Student findings:\n{}", findings.join("\n"))
        } else {
            "Student: No major issues found (needs more training)".to_string()
        };

        Ok(DomainTeachingResult {
            lesson_plan: plan,
            instruction: instruction.to_string(),
            student_questions: vec![
                format!("What should I look for when reviewing for {topic}?"),
                format!("Can you give me an example of a {topic} issue?"),
            ],
            agent_answers: vec![
                format!("Focus on the most common {topic} patterns. Use a checklist approach."),
                format!(
                    "A common {topic} issue is taking shortcuts - like using f-strings for SQL."
                ),
            ],
            student_attempt: attempt,
            scores: HashMap::new(),
        })
    }

    fn available_tools(&self) -> Vec<String> {
        vec![
            "analyze_code".into(),
            "check_style".into(),
            "detect_security_issues".into(),
            "suggest_improvements".into(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> CodeReviewAgent {
        CodeReviewAgent::with_defaults()
    }

    #[test]
    fn domain_and_name() {
        let a = agent();
        assert_eq!(a.domain(), "code_review");
        assert_eq!(a.agent_name(), "code_review_agent");
    }

    #[test]
    fn execute_empty_code() {
        let a = agent();
        let task = HashMap::from([("code".into(), serde_json::json!(""))]);
        let r = a.execute_task(&task).unwrap();
        assert!(!r.success);
    }

    #[test]
    fn execute_clean_code() {
        let a = agent();
        let task = HashMap::from([
            (
                "code".into(),
                serde_json::json!("def hello():\n    \"\"\"Greet.\"\"\"\n    print('hi')\n"),
            ),
            ("language".into(), serde_json::json!("python")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(r.success);
        let output = r.output.unwrap();
        assert!(output.get("score").is_some());
    }

    #[test]
    fn execute_code_with_issues() {
        let a = agent();
        let task = HashMap::from([
            (
                "code".into(),
                serde_json::json!("result = eval(user_input)\n"),
            ),
            ("language".into(), serde_json::json!("python")),
        ]);
        let r = a.execute_task(&task).unwrap();
        assert!(r.success);
        let output = r.output.unwrap();
        let count = output["issue_count"].as_u64().unwrap();
        assert!(count > 0);
    }

    #[test]
    fn teach_security() {
        let a = agent();
        let r = a.teach("security review", "beginner").unwrap();
        assert!(r.lesson_plan.contains("SQL injection"));
        assert!(!r.student_attempt.is_empty());
    }

    #[test]
    fn eval_levels_returned() {
        let a = agent();
        let levels = a.eval_levels();
        assert_eq!(levels.len(), 4);
    }

    #[test]
    fn available_tools_list() {
        let a = agent();
        let tools = a.available_tools();
        assert!(tools.contains(&"analyze_code".to_string()));
        assert!(tools.contains(&"detect_security_issues".to_string()));
    }
}
