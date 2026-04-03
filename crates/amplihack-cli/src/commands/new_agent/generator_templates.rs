//! Template / section generators used by [`super::generator::AgentGenerator`].

use std::fmt::Write;

use super::generator::{AgentRequirement, Complexity};

pub(super) fn titlecase_name(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.extend(chars);
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn generate_description(
    req: &AgentRequirement,
    domain: &str,
    action: &str,
) -> String {
    format!(
        "{}\n\nThis {} agent operates within the {} domain\n\
         to provide specialized functionality for {} operations.",
        req.purpose, req.suggested_type, domain, action
    )
}

pub(super) fn generate_capabilities(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        return "- General processing and analysis".to_string();
    }
    capabilities
        .iter()
        .map(|c| format!("- **{}**: Perform {} operations on input data", titlecase_name(c), c))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn generate_responsibilities(
    req: &AgentRequirement,
    complexity: Complexity,
) -> String {
    let caps = if req.capabilities.is_empty() {
        "core".to_string()
    } else {
        req.capabilities
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut lines = vec![
        format!("1. **Primary**: {}", req.purpose),
        "2. **Validation**: Ensure input data meets requirements".to_string(),
        format!("3. **Processing**: Execute {caps} operations"),
        "4. **Error Handling**: Gracefully handle failures and edge cases".to_string(),
        "5. **Reporting**: Provide clear feedback and results".to_string(),
    ];
    if complexity == Complexity::Advanced {
        lines.push("6. **Optimization**: Maximize performance and efficiency".to_string());
        lines.push("7. **Monitoring**: Track operational metrics".to_string());
        lines.push("8. **Integration**: Coordinate with other system components".to_string());
    }
    lines.join("\n")
}

pub(super) fn generate_implementation(
    req: &AgentRequirement,
    complexity: Complexity,
) -> String {
    let cap = req
        .capabilities
        .first()
        .cloned()
        .unwrap_or_else(|| "default".to_string());
    let mut out = format!(
        "### Input Processing\n\n\
         The agent accepts input in the following formats:\n\
         - Structured data (JSON, YAML)\n\
         - Text-based commands\n\
         - File paths for batch processing\n\n\
         ### Core Algorithm\n\n\
         ```python\n\
         def process(input_data):\n\
         \x20   # Validate input\n\
         \x20   validated = validate_input(input_data)\n\n\
         \x20   # Apply transformations\n\
         \x20   processed = apply_{cap}(validated)\n\n\
         \x20   # Generate output\n\
         \x20   return format_output(processed)\n\
         ```\n\n\
         ### Output Format\n\n\
         Results are returned in a structured format with:\n\
         - Status indicator\n\
         - Processed data\n\
         - Metadata (processing time, warnings, etc.)\n\
         - Error information (if applicable)"
    );
    if complexity != Complexity::Simple {
        out.push_str(
            "\n\n### Advanced Features\n\n\
             - Parallel processing for large datasets\n\
             - Caching for frequently accessed data\n\
             - Incremental processing capabilities\n\
             - Real-time streaming support",
        );
    }
    out
}

pub(super) fn generate_philosophy() -> &'static str {
    "This agent follows the amplihack philosophy of:\n\n\
     - **Ruthless Simplicity**: Start simple, add complexity only when justified\n\
     - **Modular Design**: Self-contained with clear interfaces\n\
     - **Zero-BS Implementation**: No stubs or placeholders, only working code\n\
     - **Regeneratable**: Can be rebuilt from specification\n\
     - **Trust Through Transparency**: Clear documentation and error messages\n\n\
     The agent prioritizes clarity and maintainability over premature optimization."
}

pub(super) fn generate_error_handling() -> &'static str {
    "The agent implements comprehensive error handling:\n\n\
     1. **Input Validation Errors**\n\
     \x20  - Clear messages about what's wrong\n\
     \x20  - Suggestions for correction\n\
     \x20  - Examples of valid input\n\n\
     2. **Processing Errors**\n\
     \x20  - Graceful degradation when possible\n\
     \x20  - Partial results with warnings\n\
     \x20  - Detailed error context for debugging\n\n\
     3. **Resource Errors**\n\
     \x20  - Timeout handling with configurable limits\n\
     \x20  - Memory management and cleanup\n\
     \x20  - Retry logic with exponential backoff\n\n\
     4. **Recovery Strategies**\n\
     \x20  - Automatic retry for transient failures\n\
     \x20  - Fallback to simpler processing modes\n\
     \x20  - State preservation for resumption"
}

pub(super) fn generate_performance(complexity: Complexity) -> String {
    let base = "- **Latency**: Optimized for sub-second response times\n\
                - **Throughput**: Handles standard workloads efficiently\n\
                - **Memory**: Minimal memory footprint";
    match complexity {
        Complexity::Simple => {
            format!("{base}\n- **Scalability**: Suitable for small to medium datasets")
        }
        Complexity::Standard => format!(
            "{base}\n- **Scalability**: Handles medium to large datasets\n\
             - **Caching**: Smart caching for repeated operations\n\
             - **Batching**: Efficient batch processing support"
        ),
        Complexity::Advanced => format!(
            "{base}\n- **Scalability**: Enterprise-scale data processing\n\
             - **Caching**: Multi-level caching with TTL\n\
             - **Batching**: Advanced batch processing with parallelization\n\
             - **Streaming**: Real-time stream processing capabilities\n\
             - **Resource Management**: Dynamic resource allocation\n\
             - **Monitoring**: Built-in performance metrics"
        ),
    }
}

pub(super) fn generate_dependencies(dependencies: &[String]) -> String {
    if dependencies.is_empty() {
        return "No external dependencies required.".to_string();
    }
    let mut out = "This agent depends on:\n".to_string();
    for dep in dependencies {
        let _ = writeln!(out, "- {dep}");
    }
    out.trim_end().to_string()
}

pub(super) fn generate_examples(agent_name: &str) -> String {
    format!(
        "```python\n\
         # Example 1: Basic usage\n\
         result = {name}.process(\"input data\")\n\
         print(result.status)  # \"success\"\n\n\
         # Example 2: With options\n\
         options = {{\n\
         \x20   \"validate\": True,\n\
         \x20   \"format\": \"json\",\n\
         \x20   \"verbose\": False\n\
         }}\n\
         result = {name}.process(\"input data\", options)\n\n\
         # Example 3: Batch processing\n\
         inputs = [\"data1\", \"data2\", \"data3\"]\n\
         results = {name}.process_batch(inputs)\n\
         for result in results:\n\
         \x20   if result.success:\n\
         \x20       print(f\"Processed: {{result.data}}\")\n\
         ```",
        name = agent_name
    )
}

pub(super) fn generate_testing(req: &AgentRequirement) -> String {
    format!(
        "### Test Coverage\n\n\
         - Unit tests for all {cap_count} capabilities\n\
         - Integration tests with common workflows\n\
         - Edge case handling tests\n\
         - Performance benchmarks\n\
         - Security validation tests\n\n\
         ### Running Tests\n\n\
         ```bash\n\
         # Run all tests\n\
         pytest tests/test_{name}.py\n\n\
         # Run specific test category\n\
         pytest tests/test_{name}.py::TestValidation\n\n\
         # Run with coverage\n\
         pytest --cov={name} tests/\n\
         ```\n\n\
         ### Test Data\n\n\
         Test fixtures are provided in `tests/fixtures/{name}/`",
        name = req.name,
        cap_count = req.capabilities.len()
    )
}

/// Generate a pytest test file scaffold for an agent.
pub(super) fn generate_test_file(req: &AgentRequirement) -> String {
    let class_name: String = req
        .name
        .split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => {
                    let mut s = f.to_uppercase().to_string();
                    s.extend(c);
                    s
                }
                None => String::new(),
            }
        })
        .collect();
    let display = titlecase_name(&req.name);
    let name = &req.name;
    let mut out = String::with_capacity(2048);
    // Use regular strings, not raw strings, to avoid Rust 2024 prefix issues
    let _ = write!(
        out,
        "\"\"\"\nTests for {display}\n\n\
         These are functional tests that verify the agent markdown file structure\n\
         and content without importing it as a Python module.\n\
         \"\"\"\n\n\
         import pytest\n\
         from pathlib import Path\n\n\n\
         class Test{class_name}:\n\
         \x20   \"\"\"Test suite for {name} agent bundle.\"\"\"\n\n"
    );
    let _ = write!(
        out,
        "\x20   def test_agent_file_exists(self):\n\
         \x20       \"\"\"Test that agent markdown file exists.\"\"\"\n\
         \x20       agents_dir = Path(__file__).parent.parent / \"agents\"\n\
         \x20       agent_file = agents_dir / \"{name}.md\"\n\
         \x20       assert agent_file.exists(), \
         f\"Agent file should exist at {{agent_file}}\"\n\n"
    );
    let _ = write!(
        out,
        "\x20   def test_agent_content_structure(self):\n\
         \x20       \"\"\"Test agent file has required sections.\"\"\"\n\
         \x20       agents_dir = Path(__file__).parent.parent / \"agents\"\n\
         \x20       agent_file = agents_dir / \"{name}.md\"\n\
         \x20       content = agent_file.read_text()\n\
         \x20       assert \"## \" + \"Role\" in content, \"Should have Role section\"\n\
         \x20       assert \"## \" + \"Capabilities\" in content, \
         \"Should have Capabilities section\"\n\
         \x20       assert \"## \" + \"Implementation\" in content, \
         \"Should have Implementation section\"\n\n"
    );
    let _ = write!(
        out,
        "\x20   def test_agent_content_not_empty(self):\n\
         \x20       \"\"\"Test agent file has substantial content.\"\"\"\n\
         \x20       agents_dir = Path(__file__).parent.parent / \"agents\"\n\
         \x20       agent_file = agents_dir / \"{name}.md\"\n\
         \x20       content = agent_file.read_text()\n\
         \x20       assert len(content) > 500, \
         \"Agent file should have substantial content\"\n"
    );
    out
}

/// Generate extended documentation for an agent.
pub(super) fn generate_extended_docs(req: &AgentRequirement) -> String {
    let display = titlecase_name(&req.name);
    format!(
        "# {display} - Extended Documentation\n\n\
         ## Overview\n\n{purpose}\n\n\
         ## Architecture\n\n\
         The agent follows a pipeline architecture:\n\n\
         1. **Input Stage**: Receives and validates input\n\
         2. **Processing Stage**: Applies core logic\n\
         3. **Output Stage**: Formats and returns results\n\n\
         ## Configuration\n\n\
         Configuration options can be provided via:\n\
         - Environment variables\n\
         - Configuration files (JSON/YAML)\n\
         - Runtime parameters\n\n\
         ## Integration Guide\n\n\
         ### Using the Agent with Claude Code\n\n\
         This agent bundle is designed to be used with Claude Code.\n\n\
         **Add to your .claude/agents directory:**\n\n\
         ```bash\ncp agents/{name}.md /path/to/your/project/.claude/agents/\n```\n\n\
         **Reference in Claude Code:**\n\n\
         ```markdown\n@.claude/agents/{name}.md\n```\n\n\
         ## Troubleshooting\n\n\
         1. **File Not Found**: Ensure the agent file is in the agents/ directory\n\
         2. **Validation Error**: Check that the agent markdown has required sections\n\
         3. **Integration Issues**: Verify Claude Code can access the .claude/agents directory\n\n\
         ## Version History\n\n- v1.0.0: Initial release",
        name = req.name,
        purpose = req.purpose
    )
}
