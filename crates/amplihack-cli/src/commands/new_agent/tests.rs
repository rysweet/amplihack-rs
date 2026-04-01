use super::*;
use tempfile::TempDir;

fn write_prompt(dir: &Path, content: &str) -> PathBuf {
    let p = dir.join("prompt.md");
    fs::write(&p, content).unwrap();
    p
}

#[test]
fn test_sanitize_bundle_name_basic() {
    assert_eq!(sanitize_bundle_name("my agent", "-agent"), "my-agent-agent");
    assert_eq!(sanitize_bundle_name("TEST_NAME", ""), "test-name");
    assert_eq!(sanitize_bundle_name("", "-agent"), "agent-agent");
}

#[test]
fn test_sanitize_bundle_name_long() {
    let long = "a".repeat(60);
    let result = sanitize_bundle_name(&long, "");
    assert!(result.len() <= 50, "got len={}", result.len());
    assert!(result.len() >= 3);
}

#[test]
fn test_sanitize_bundle_name_special_chars() {
    let result = sanitize_bundle_name("Test@#$Name!!!", "-agent");
    assert!(!result.contains('@'));
    assert!(!result.contains('#'));
    assert!(!result.contains('$'));
    assert!(!result.contains('!'));
}

#[test]
fn test_analyze_prompt_basic() {
    let prompt = "# Build a data pipeline\n\nProcess CSV files and generate reports.";
    let goal = analysis::analyze_prompt(prompt).unwrap();
    assert!(!goal.goal.is_empty());
    assert!(!goal.domain.is_empty());
    assert!(!goal.complexity.is_empty());
}

#[test]
fn test_analyze_prompt_empty_fails() {
    assert!(analysis::analyze_prompt("").is_err());
    assert!(analysis::analyze_prompt("   ").is_err());
}

#[test]
fn test_domain_classification() {
    assert_eq!(
        analysis::classify_domain("scan for security vulnerabilities"),
        "security-analysis"
    );
    assert_eq!(
        analysis::classify_domain("deploy the application to production"),
        "deployment"
    );
    assert_eq!(analysis::classify_domain("test the API endpoints"), "testing");
    assert_eq!(
        analysis::classify_domain("process and transform data"),
        "data-processing"
    );
    assert_eq!(analysis::classify_domain("something completely generic"), "general");
}

#[test]
fn test_complexity_detection() {
    assert_eq!(analysis::determine_complexity("simple one step task"), "simple");
    assert_eq!(
        analysis::determine_complexity("complex distributed multi-stage pipeline"),
        "complex"
    );
    // word count heuristic
    let long_prompt = "word ".repeat(200);
    assert_eq!(analysis::determine_complexity(&long_prompt), "complex");
}

#[test]
fn test_e2e_creates_expected_files() {
    let tmp = TempDir::new().unwrap();
    let prompt_path =
        write_prompt(tmp.path(), "# Automate deployment\n\nDeploy to production.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        None,
        None,
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    // find the agent dir (name is auto-generated)
    let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
    assert_eq!(entries.len(), 1, "expected exactly one agent directory");
    let agent_dir = entries[0].as_ref().unwrap().path();

    assert!(agent_dir.join("prompt.md").exists(), "prompt.md missing");
    assert!(agent_dir.join("main.py").exists(), "main.py missing");
    assert!(agent_dir.join("README.md").exists(), "README.md missing");
    assert!(
        agent_dir.join("agent_config.json").exists(),
        "agent_config.json missing"
    );
    assert!(
        agent_dir.join("requirements.txt").exists(),
        "requirements.txt missing"
    );
    assert!(
        agent_dir
            .join(".claude")
            .join("context")
            .join("goal.json")
            .exists()
    );
    assert!(
        agent_dir
            .join(".claude")
            .join("context")
            .join("execution_plan.json")
            .exists()
    );
}

#[test]
fn test_memory_mode_writes_artifacts() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Process data\n\nAnalyze CSV files.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("mem-agent"),
        None,
        false,
        true,
        "copilot",
        false,
        false,
    )
    .unwrap();

    let agent_dir = out.join("mem-agent");
    assert!(
        agent_dir.join("memory_config.yaml").exists(),
        "memory_config.yaml missing"
    );
    assert!(
        agent_dir.join("memory").join(".gitignore").exists(),
        "memory/.gitignore missing"
    );
    let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
    assert!(
        reqs.contains("amplihack-memory-lib"),
        "memory dep missing from requirements.txt"
    );
}

#[test]
fn test_multi_agent_mode_writes_sub_agent_configs() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(
        tmp.path(),
        "# Orchestrate multiple services\n\nCoordinate deployment.",
    );
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("multi-agent"),
        None,
        false,
        false,
        "claude",
        true,
        false,
    )
    .unwrap();

    let agent_dir = out.join("multi-agent");
    assert!(
        agent_dir
            .join("sub_agents")
            .join("coordinator.yaml")
            .exists()
    );
    assert!(
        agent_dir
            .join("sub_agents")
            .join("memory_agent.yaml")
            .exists()
    );
    assert!(agent_dir.join("sub_agents").join("spawner.yaml").exists());
    assert!(agent_dir.join("sub_agents").join("__init__.py").exists());
    let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
    assert!(
        reqs.contains("pyyaml"),
        "pyyaml missing from requirements.txt"
    );
}

#[test]
fn test_enable_spawning_implies_multi_agent() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Spawn workers\n\nDynamic task spawning.");
    let out = tmp.path().join("out");

    // enable_spawning=true, multi_agent=false — should auto-enable multi-agent
    run_new(
        &prompt_path,
        Some(&out),
        Some("spawner-test"),
        None,
        false,
        false,
        "copilot",
        false,
        true,
    )
    .unwrap();

    let agent_dir = out.join("spawner-test");
    // sub_agents directory should exist (multi-agent was auto-enabled)
    assert!(agent_dir.join("sub_agents").exists());
    // spawner.yaml should have enabled: true
    let spawner =
        fs::read_to_string(agent_dir.join("sub_agents").join("spawner.yaml")).unwrap();
    assert!(spawner.contains("enabled: true"));
}

#[test]
fn test_missing_skills_dir_falls_back_to_generic() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Process data\n\nTransform and analyze.");
    let out = tmp.path().join("out");

    // Pass a non-existent skills dir
    let nonexistent = tmp.path().join("no_skills_here");
    run_new(
        &prompt_path,
        Some(&out),
        Some("fallback-test"),
        Some(&nonexistent),
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    let agents_dir = out.join("fallback-test").join(".claude").join("agents");
    let skill_files: Vec<_> = fs::read_dir(&agents_dir).unwrap().collect();
    assert!(
        !skill_files.is_empty(),
        "expected at least one generic skill file"
    );
}

#[test]
fn test_custom_name_is_sanitized() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "Deploy to staging.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("My Custom Name!!!"),
        None,
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    // Sanitized name should exist (special chars stripped)
    let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let dir_name = entries[0].as_ref().unwrap().file_name();
    let dir_str = dir_name.to_string_lossy();
    assert!(!dir_str.contains('!'), "name should be sanitized");
}
