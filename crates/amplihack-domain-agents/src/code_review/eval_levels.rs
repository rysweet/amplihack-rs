//! Evaluation levels for the Code Review agent.
//!
//! Ports `domain_agents/code_review/eval_levels.py`: L1-L4 evaluation
//! scenarios covering bug detection, style, security, and architecture.

use crate::base::{EvalLevel, EvalScenario};
use std::collections::HashMap;

pub fn get_eval_levels() -> Vec<EvalLevel> {
    vec![l1(), l2(), l3(), l4()]
}

fn l1() -> EvalLevel {
    EvalLevel::new(
        "L1",
        "Basic Detection",
        "Finds obvious bugs",
        vec![
            EvalScenario {
                scenario_id: "L1-001".into(),
                name: "Undefined variable".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!("def calc(x):\n    return x + y\n"),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must identify y is not defined.".into(),
            },
            EvalScenario {
                scenario_id: "L1-002".into(),
                name: "Division by zero risk".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!("def divide(a, b):\n    return a / b\n"),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must identify division by zero risk.".into(),
            },
            EvalScenario {
                scenario_id: "L1-003".into(),
                name: "Index out of range".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!("def get_last(items):\n    return items[len(items)]\n"),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must identify off-by-one error.".into(),
            },
        ],
    )
}

fn l2() -> EvalLevel {
    EvalLevel::new("L2", "Style & Quality", "Style violations and code quality", vec![
        EvalScenario {
            scenario_id: "L2-001".into(),
            name: "Naming conventions".into(),
            input_data: HashMap::from([
                ("code".into(), serde_json::json!("def calculateTotal(itemList):\n    myVar = 0\n    for Item in itemList:\n        myVar += Item\n    return myVar\n")),
                ("language".into(), serde_json::json!("python")),
            ]),
            expected_output: HashMap::from([
                ("min_issue_count".into(), serde_json::json!(1)),
            ]),
            grading_rubric: "Must identify camelCase naming.".into(),
        },
        EvalScenario {
            scenario_id: "L2-002".into(),
            name: "Missing docstrings".into(),
            input_data: HashMap::from([
                ("code".into(), serde_json::json!("class DataProcessor:\n    def process(self, data):\n        return [x*2 for x in data if x > 0]\n")),
                ("language".into(), serde_json::json!("python")),
            ]),
            expected_output: HashMap::from([
                ("min_issue_count".into(), serde_json::json!(1)),
            ]),
            grading_rubric: "Must note missing docstrings.".into(),
        },
        EvalScenario {
            scenario_id: "L2-003".into(),
            name: "Bare except".into(),
            input_data: HashMap::from([
                ("code".into(), serde_json::json!("def read_file(path):\n    try:\n        with open(path) as f:\n            return f.read()\n    except:\n        return None\n")),
                ("language".into(), serde_json::json!("python")),
            ]),
            expected_output: HashMap::from([
                ("min_issue_count".into(), serde_json::json!(1)),
            ]),
            grading_rubric: "Must flag bare except.".into(),
        },
    ]).with_threshold(0.6)
}

fn l3() -> EvalLevel {
    EvalLevel::new(
        "L3",
        "Security Review",
        "Security vulnerabilities",
        vec![
            EvalScenario {
                scenario_id: "L3-001".into(),
                name: "SQL injection".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!(
                            "def get_user(cursor, name):\n    cursor.execute(f\"SELECT * FROM users WHERE name = '{name}'\")\n    return cursor.fetchone()\n"
                        ),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must find SQL injection.".into(),
            },
            EvalScenario {
                scenario_id: "L3-002".into(),
                name: "Hardcoded secret".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!(
                            "API_KEY = \"sk-1234567890\"\nDATABASE_PASSWORD = \"hunter2\"\n"
                        ),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must flag hardcoded secrets.".into(),
            },
            EvalScenario {
                scenario_id: "L3-003".into(),
                name: "Eval usage".into(),
                input_data: HashMap::from([
                    (
                        "code".into(),
                        serde_json::json!("def calculate(expr):\n    return eval(expr)\n"),
                    ),
                    ("language".into(), serde_json::json!("python")),
                ]),
                expected_output: HashMap::from([("min_issue_count".into(), serde_json::json!(1))]),
                grading_rubric: "Must flag eval().".into(),
            },
        ],
    )
}

fn l4() -> EvalLevel {
    EvalLevel::new("L4", "Architecture", "Structural improvements", vec![
        EvalScenario {
            scenario_id: "L4-001".into(),
            name: "God class".into(),
            input_data: HashMap::from([
                ("code".into(), serde_json::json!(
                    format!("class AppManager:\n{}", (0..10).map(|i| format!("    def method_{i}(self): pass\n")).collect::<String>())
                )),
                ("language".into(), serde_json::json!("python")),
            ]),
            expected_output: HashMap::from([
                ("min_issue_count".into(), serde_json::json!(1)),
            ]),
            grading_rubric: "Must identify god class.".into(),
        },
        EvalScenario {
            scenario_id: "L4-002".into(),
            name: "Duplicated logic".into(),
            input_data: HashMap::from([
                ("code".into(), serde_json::json!("def process(orders):\n    for o in orders:\n        t = 0\n        for i in o.items:\n            t += i.price * i.qty\n        o.total = t\n\ndef calc_tax(orders):\n    for o in orders:\n        t = 0\n        for i in o.items:\n            t += i.price * i.qty\n        o.tax = t * 0.1\n")),
                ("language".into(), serde_json::json!("python")),
            ]),
            expected_output: HashMap::from([
                ("min_issue_count".into(), serde_json::json!(1)),
            ]),
            grading_rubric: "Must identify duplicated logic.".into(),
        },
    ]).with_threshold(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_levels_count() {
        let levels = get_eval_levels();
        assert_eq!(levels.len(), 4);
    }

    #[test]
    fn eval_level_ids() {
        let levels = get_eval_levels();
        let ids: Vec<&str> = levels.iter().map(|l| l.level_id.as_str()).collect();
        assert_eq!(ids, vec!["L1", "L2", "L3", "L4"]);
    }

    #[test]
    fn l1_has_three_scenarios() {
        let levels = get_eval_levels();
        assert_eq!(levels[0].scenarios.len(), 3);
    }

    #[test]
    fn thresholds_are_set() {
        let levels = get_eval_levels();
        assert!((levels[0].passing_threshold - 0.7).abs() < f64::EPSILON);
        assert!((levels[1].passing_threshold - 0.6).abs() < f64::EPSILON);
        assert!((levels[3].passing_threshold - 0.5).abs() < f64::EPSILON);
    }
}
