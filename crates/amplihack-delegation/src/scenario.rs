use crate::models::{ScenarioCategory, TestScenario};

// ---------------------------------------------------------------------------
// ScenarioGenerator
// ---------------------------------------------------------------------------

/// Generates test scenarios from a goal and success criteria.
///
/// Ported from the Python `GadugiScenarioGenerator`. Detects the domain of the
/// goal (API, auth, performance, …) and produces category-specific scenarios.
#[derive(Debug, Default)]
pub struct ScenarioGenerator;

impl ScenarioGenerator {
    /// Create a new generator.
    pub fn new() -> Self {
        Self
    }

    /// Produce test scenarios for the given `goal` and `success_criteria`.
    ///
    /// Domain keywords in `goal` and `criteria` determine which optional
    /// categories (security, performance) are included.
    pub fn generate_scenarios(&self, goal: &str, success_criteria: &str) -> Vec<TestScenario> {
        let combined = format!("{} {}", goal, success_criteria).to_lowercase();

        let is_api = Self::is_api(&combined);
        let is_auth = Self::is_auth(&combined);
        let is_perf = Self::is_perf(&combined);
        let is_pagination = Self::is_pagination(&goal.to_lowercase());

        let mut scenarios = Vec::new();

        scenarios.extend(self.generate_happy_path(goal, success_criteria, is_api));
        scenarios.extend(self.generate_error_handling(goal, success_criteria, is_api));
        scenarios.extend(self.generate_boundary(goal, success_criteria, is_api, is_pagination));

        if is_auth || combined.contains("security") || combined.contains("admin") {
            scenarios.extend(self.generate_security(goal, success_criteria));
        }
        if is_perf {
            scenarios.extend(self.generate_performance(goal, success_criteria));
        }

        scenarios.extend(self.generate_integration(goal, success_criteria));

        scenarios
    }

    // -- domain detection ---------------------------------------------------

    fn is_api(text: &str) -> bool {
        const KEYWORDS: &[&str] = &[
            "api", "endpoint", "rest", "http", "post", "get", "put", "delete",
        ];
        KEYWORDS.iter().any(|kw| text.contains(kw))
    }

    fn is_auth(text: &str) -> bool {
        const KEYWORDS: &[&str] = &[
            "auth",
            "login",
            "token",
            "jwt",
            "password",
            "permission",
            "secure",
        ];
        KEYWORDS.iter().any(|kw| text.contains(kw))
    }

    fn is_perf(text: &str) -> bool {
        const KEYWORDS: &[&str] = &["performance", "load", "scale"];
        KEYWORDS.iter().any(|kw| text.contains(kw))
    }

    fn is_pagination(goal_lower: &str) -> bool {
        goal_lower.contains("paginat") || goal_lower.contains("list")
    }

    // -- happy path ---------------------------------------------------------

    fn generate_happy_path(&self, goal: &str, _criteria: &str, is_api: bool) -> Vec<TestScenario> {
        let mut out = Vec::new();

        if is_api {
            out.push(TestScenario {
                name: "Valid API request succeeds".into(),
                category: ScenarioCategory::HappyPath,
                description: format!("Send a valid request to achieve: {goal}"),
                preconditions: vec![
                    "API server is running".into(),
                    "Valid credentials available".into(),
                ],
                steps: vec![
                    "Prepare valid request payload".into(),
                    "Send request to API endpoint".into(),
                    "Verify response status is 2xx".into(),
                    "Validate response body schema".into(),
                ],
                expected_outcome: "API returns successful response with correct data".into(),
                priority: "high".into(),
                tags: vec!["api".into(), "happy-path".into()],
            });
            out.push(TestScenario {
                name: "Complete API workflow succeeds".into(),
                category: ScenarioCategory::HappyPath,
                description: format!("Execute the complete workflow for: {goal}"),
                preconditions: vec!["System is in clean state".into()],
                steps: vec![
                    "Initialize required resources".into(),
                    "Execute primary workflow steps".into(),
                    "Verify intermediate results".into(),
                    "Confirm final state is correct".into(),
                ],
                expected_outcome: "Entire workflow completes without errors".into(),
                priority: "high".into(),
                tags: vec!["api".into(), "workflow".into()],
            });
        } else {
            out.push(TestScenario {
                name: "Basic functionality works".into(),
                category: ScenarioCategory::HappyPath,
                description: format!("Verify basic functionality for: {goal}"),
                preconditions: vec!["System is properly configured".into()],
                steps: vec![
                    "Set up required preconditions".into(),
                    "Execute the primary action".into(),
                    "Verify the expected outcome".into(),
                ],
                expected_outcome: "System behaves as specified in success criteria".into(),
                priority: "high".into(),
                tags: vec!["basic".into(), "happy-path".into()],
            });
        }
        out
    }

    // -- error handling -----------------------------------------------------

    fn generate_error_handling(
        &self,
        goal: &str,
        _criteria: &str,
        is_api: bool,
    ) -> Vec<TestScenario> {
        let mut out = Vec::new();

        if is_api {
            out.push(TestScenario {
                name: "Invalid input returns error".into(),
                category: ScenarioCategory::ErrorHandling,
                description: format!("Submit invalid input for: {goal}"),
                preconditions: vec!["API server is running".into()],
                steps: vec![
                    "Prepare request with invalid data".into(),
                    "Send request to API endpoint".into(),
                    "Verify error response status (4xx)".into(),
                    "Check error message is descriptive".into(),
                ],
                expected_outcome: "API returns appropriate error response".into(),
                priority: "high".into(),
                tags: vec!["api".into(), "error".into()],
            });
            out.push(TestScenario {
                name: "Malformed request is rejected".into(),
                category: ScenarioCategory::ErrorHandling,
                description: format!("Send malformed request for: {goal}"),
                preconditions: vec!["API server is running".into()],
                steps: vec![
                    "Prepare malformed request body".into(),
                    "Send request to API endpoint".into(),
                    "Verify 400 Bad Request response".into(),
                ],
                expected_outcome: "API rejects malformed input gracefully".into(),
                priority: "medium".into(),
                tags: vec!["api".into(), "malformed".into()],
            });
        } else {
            out.push(TestScenario {
                name: "Invalid input handled gracefully".into(),
                category: ScenarioCategory::ErrorHandling,
                description: format!("Provide invalid input for: {goal}"),
                preconditions: vec!["System is running".into()],
                steps: vec![
                    "Prepare invalid input data".into(),
                    "Submit invalid input to the system".into(),
                    "Verify error is reported clearly".into(),
                ],
                expected_outcome: "System handles invalid input without crashing".into(),
                priority: "high".into(),
                tags: vec!["error".into(), "validation".into()],
            });
        }

        // Always add missing-data scenario.
        out.push(TestScenario {
            name: "Missing required data is rejected".into(),
            category: ScenarioCategory::ErrorHandling,
            description: format!("Omit required fields for: {goal}"),
            preconditions: vec!["System is running".into()],
            steps: vec![
                "Prepare request with missing required fields".into(),
                "Submit to the system".into(),
                "Verify rejection with clear error message".into(),
            ],
            expected_outcome: "System rejects incomplete data with informative error".into(),
            priority: "high".into(),
            tags: vec!["error".into(), "missing-data".into()],
        });

        out
    }

    // -- boundary conditions ------------------------------------------------

    fn generate_boundary(
        &self,
        goal: &str,
        _criteria: &str,
        is_api: bool,
        is_pagination: bool,
    ) -> Vec<TestScenario> {
        let mut out = Vec::new();

        let ctx = if is_api { "API" } else { "system" };

        out.push(TestScenario {
            name: "Empty input handled".into(),
            category: ScenarioCategory::BoundaryConditions,
            description: format!("Submit empty input for: {goal}"),
            preconditions: vec![format!("{ctx} is running")],
            steps: vec![
                "Prepare empty input".into(),
                format!("Submit to {ctx}"),
                "Verify appropriate response".into(),
            ],
            expected_outcome: format!("{ctx} handles empty input without errors"),
            priority: "medium".into(),
            tags: vec!["boundary".into(), "empty".into()],
        });

        out.push(TestScenario {
            name: "Maximum size input handled".into(),
            category: ScenarioCategory::BoundaryConditions,
            description: format!("Submit maximum-size input for: {goal}"),
            preconditions: vec![format!("{ctx} is running")],
            steps: vec![
                "Prepare maximum-size input data".into(),
                format!("Submit to {ctx}"),
                "Verify system handles large input correctly".into(),
            ],
            expected_outcome: format!("{ctx} processes or rejects oversized input gracefully"),
            priority: "medium".into(),
            tags: vec!["boundary".into(), "max-size".into()],
        });

        if is_pagination {
            out.push(TestScenario {
                name: "Pagination with zero results".into(),
                category: ScenarioCategory::BoundaryConditions,
                description: format!("Request page when no results exist for: {goal}"),
                preconditions: vec!["No matching data exists".into()],
                steps: vec![
                    "Send paginated request".into(),
                    "Verify empty result set".into(),
                    "Check pagination metadata is correct".into(),
                ],
                expected_outcome: "Returns empty list with correct pagination info".into(),
                priority: "medium".into(),
                tags: vec!["boundary".into(), "pagination".into()],
            });
        }

        out
    }

    // -- security -----------------------------------------------------------

    fn generate_security(&self, goal: &str, _criteria: &str) -> Vec<TestScenario> {
        vec![
            TestScenario {
                name: "Unauthorized access blocked".into(),
                category: ScenarioCategory::Security,
                description: format!("Attempt access without credentials for: {goal}"),
                preconditions: vec!["No authentication token provided".into()],
                steps: vec![
                    "Send request without authentication".into(),
                    "Verify 401 Unauthorized response".into(),
                    "Confirm no data is leaked".into(),
                ],
                expected_outcome: "Unauthorized requests are rejected".into(),
                priority: "high".into(),
                tags: vec!["security".into(), "auth".into()],
            },
            TestScenario {
                name: "Insufficient permissions denied".into(),
                category: ScenarioCategory::Security,
                description: format!("Attempt privileged action without permission for: {goal}"),
                preconditions: vec!["Authenticated with low-privilege account".into()],
                steps: vec![
                    "Authenticate with limited permissions".into(),
                    "Attempt privileged operation".into(),
                    "Verify 403 Forbidden response".into(),
                ],
                expected_outcome: "System enforces permission boundaries".into(),
                priority: "high".into(),
                tags: vec!["security".into(), "permissions".into()],
            },
            TestScenario {
                name: "SQL injection prevented".into(),
                category: ScenarioCategory::Security,
                description: format!("Attempt SQL injection for: {goal}"),
                preconditions: vec!["System is running".into()],
                steps: vec![
                    "Prepare input with SQL injection payload".into(),
                    "Submit to the system".into(),
                    "Verify injection is neutralized".into(),
                    "Confirm no data exposure".into(),
                ],
                expected_outcome: "System sanitizes input and prevents injection".into(),
                priority: "high".into(),
                tags: vec!["security".into(), "injection".into()],
            },
        ]
    }

    // -- performance --------------------------------------------------------

    fn generate_performance(&self, goal: &str, _criteria: &str) -> Vec<TestScenario> {
        vec![TestScenario {
            name: "System handles concurrent requests".into(),
            category: ScenarioCategory::Performance,
            description: format!("Verify concurrent request handling for: {goal}"),
            preconditions: vec!["System is running under normal conditions".into()],
            steps: vec![
                "Prepare multiple concurrent requests".into(),
                "Send requests simultaneously".into(),
                "Measure response times".into(),
                "Verify all requests complete successfully".into(),
            ],
            expected_outcome: "System handles concurrency without degradation".into(),
            priority: "medium".into(),
            tags: vec!["performance".into(), "concurrency".into()],
        }]
    }

    // -- integration --------------------------------------------------------

    fn generate_integration(&self, goal: &str, _criteria: &str) -> Vec<TestScenario> {
        vec![TestScenario {
            name: "End-to-end workflow completes".into(),
            category: ScenarioCategory::Integration,
            description: format!("Execute full end-to-end workflow for: {goal}"),
            preconditions: vec!["All system components are available".into()],
            steps: vec![
                "Initialize system components".into(),
                "Execute workflow from start to finish".into(),
                "Verify all intermediate states".into(),
                "Confirm final outcome matches expectations".into(),
            ],
            expected_outcome: "Complete workflow succeeds end-to-end".into(),
            priority: "high".into(),
            tags: vec!["integration".into(), "e2e".into()],
        }]
    }
}

// Tests in tests/scenario_test.rs to stay under 400 lines.
