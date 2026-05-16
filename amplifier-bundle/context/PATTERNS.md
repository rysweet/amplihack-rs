# Development Patterns & Solutions

This document captures proven patterns and solutions for clean design and robust development. It serves as a quick reference for recurring challenges.

## Pattern Curation Philosophy

This document maintains **14 foundational patterns** that apply across most amplihack development.

**Patterns are kept when they:**

1. Solve recurring problems (used 3+ times in real PRs)
2. Apply broadly across multiple agent types and scenarios
3. Represent non-obvious solutions with working code
4. Prevent costly errors or enable critical capabilities

**Patterns are removed when they:**

- Become project-specific (better suited for PROJECT.md or DISCOVERIES.md)
- Are one-time solutions (preserved in git history)
- Are obvious applications of existing patterns
- Haven't been referenced in 6+ months

**Trust in Emergence**: Removed patterns can re-emerge when needed. See git history for context: `git log -p .claude/context/PATTERNS.md`

**This refactoring (2024-11):** Reduced from 24 to 14 patterns (74% reduction) based on usage analysis and philosophy compliance. Removed patterns include: CI Failure Rapid Diagnosis, Incremental Processing, Configuration Single Source of Truth, Parallel Task Execution (covered in CLAUDE.md), Multi-Layer Security Sanitization, Reflection-Driven Self-Improvement, Unified Validation Flow, Modular User Visibility, and others that were either too specific or better documented elsewhere.

## Core Architecture Patterns

### Pattern: Bricks & Studs Module Design with Clear Public API

> **Philosophy Reference**: See @~/.amplihack/.claude/context/PHILOSOPHY.md "The Brick Philosophy for AI Development" for the philosophical foundation of this pattern.

**Challenge**: Modules become tightly coupled, making them hard to regenerate or replace.

**Solution**: Design modules as self-contained "bricks" with clear "studs" (public API) using `pub` visibility.

```rust
//! Module-level documentation comment documents philosophy and public API.
//!
//! # Philosophy
//! - Single responsibility
//! - Standard library only (when possible)
//! - Self-contained and regeneratable
//!
//! # Public API (the "studs")
//! - [`MainClass`]: Primary functionality
//! - [`helper_function`]: Utility function
//! - [`CONSTANT`]: Configuration value

// ... implementation ...

// `pub` visibility defines the public interface
pub struct MainClass { /* ... */ }
pub fn helper_function() { /* ... */ }
pub const CONSTANT: &str = "value";
```

**Module Structure**:

```
module_name/
├── mod.rs            # Public interface via pub exports
├── README.md         # Contract specification
├── core.rs           # Implementation
├── tests/            # Test the contract
└── examples/         # Working examples
```

**Key Points**:

- Module-level documentation comment documents philosophy and public API
- `pub` visibility defines the public interface explicitly
- Standard library only for core utilities (avoid circular dependencies)
- Tests verify the contract, not implementation details

### Pattern: Zero-BS Implementation

> **Philosophy Reference**: See @~/.amplihack/.claude/context/PHILOSOPHY.md "Zero-BS Implementations" section for the core principle behind this pattern.

**Challenge**: Avoiding stub code and placeholders that serve no purpose.

**Solution**: Every function must work or not exist.

```rust
// BAD - Stub that does nothing
fn process_payment(amount: f64) -> Result<(), Box<dyn std::error::Error>> {
    // TODO: Implement Stripe integration
    unimplemented!("Coming soon")
}

// GOOD - Working implementation
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::Utc;

#[derive(Debug, Serialize, Deserialize)]
struct Payment {
    amount: f64,
    timestamp: String,
    id: String,
}

fn process_payment(amount: f64, payments_file: &str) -> Result<Payment, Box<dyn std::error::Error>> {
    let payment = Payment {
        amount,
        timestamp: Utc::now().to_rfc3339(),
        id: Uuid::new_v4().to_string(),
    };

    let mut payments: Vec<Payment> = if Path::new(payments_file).exists() {
        let data = fs::read_to_string(payments_file)?;
        serde_json::from_str(&data)?
    } else {
        Vec::new()
    };

    payments.push(payment);
    let last = payments.last().unwrap().clone();
    fs::write(payments_file, serde_json::to_string_pretty(&payments)?)?;
    Ok(last)
}
```

**Key Points**:

- Every function must work or not exist
- Use files instead of external services initially
- No TODOs without working code
- Start simple, add complexity when needed

## API & Integration Patterns

### Pattern: API Validation Before Implementation

**Challenge**: Invalid API calls cause immediate failures. Wrong model names, missing imports, or incorrect types lead to 20-30 min debug cycles.

**Solution**: Validate APIs before implementation using official documentation.

**Validation Checklist**:

1. **Model/LLM APIs**: Check model name format, verify parameters, test minimal example
2. **Imports/Libraries**: Verify module exists, check function signatures
3. **Services/Config**: Verify endpoints, check response format
4. **Error Handling**: Plan for rate limits, timeouts, specific error types

```rust
// WRONG - assumptions without validation
let client = Anthropic::new();
let message = client.messages().create(
    "claude-3-5-sonnet-20241022",  // ❌ Not verified
    "1024",                         // ❌ Wrong type (should be u32)
    &[Message::user(prompt)],
)?;

// RIGHT - validated against docs
const VALID_MODELS: &[&str] = &["claude-3-opus-20240229", "claude-3-sonnet-20241022"];
let model = "claude-3-sonnet-20241022";  // ✓ Verified
let max_tokens: u32 = 1024;              // ✓ Correct type

if !VALID_MODELS.contains(&model) {
    return Err(format!("Invalid model: {model}").into());
}

let message = client.messages().create(
    model,
    max_tokens,
    &[Message::user(prompt)],
).map_err(|e| format!("API call failed: {e}"))?;
```

**Key Points**:

- 5-10 min validation prevents 20-30 min debug cycles
- Use official documentation as source of truth
- Test imports and minimal examples before full implementation

### Pattern: Claude Code SDK Integration

**Challenge**: Integrating Claude Code SDK requires proper environment setup and timeout handling.

**Solution**:

```rust
use claude_code_sdk::{ClaudeCodeOptions, query_async};
use tokio::time::{timeout, Duration};

async fn extract_with_claude_sdk(
    prompt: &str,
    timeout_seconds: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    /// Extract using Claude Code SDK with proper timeout handling
    let options = ClaudeCodeOptions {
        system_prompt: Some("Extract information...".to_string()),
        max_turns: Some(1),
        ..Default::default()
    };

    match timeout(
        Duration::from_secs(timeout_seconds),
        query_async(prompt, options),
    ).await {
        Ok(Ok(messages)) => {
            let mut response = String::new();
            for message in messages {
                if let Some(content) = message.content_text() {
                    response.push_str(&content);
                }
            }
            Ok(response)
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            eprintln!("Claude Code SDK timed out after {timeout_seconds} seconds");
            Ok(String::new())
        }
    }
}
```

**Key Points**:

- 120-second timeout is optimal
- SDK only works in Claude Code environment
- Handle markdown in responses

## Error Handling & Reliability Patterns

### Pattern: Safe Subprocess Wrapper with Comprehensive Error Handling

**Challenge**: Subprocess calls fail with cryptic error messages. Different error types need different user guidance.

**Solution**: Create a safe subprocess wrapper with user-friendly, actionable error messages.

```rust
use std::process::Command;
use std::time::Duration;

struct SubprocessResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn safe_subprocess_call(
    cmd: &[&str],
    context: &str,
    timeout_secs: Option<u64>,
) -> SubprocessResult {
    /// Safely execute subprocess with comprehensive error handling.
    let (program, args) = match cmd.split_first() {
        Some((p, a)) => (*p, a),
        None => return SubprocessResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "Empty command".to_string(),
        },
    };

    let result = Command::new(program)
        .args(args)
        .output();

    match result {
        Ok(output) => SubprocessResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut error_msg = format!("Command not found: {program}\n");
            if !context.is_empty() {
                error_msg.push_str(&format!("Context: {context}\n"));
            }
            error_msg.push_str("Please ensure the tool is installed and in your PATH.");
            SubprocessResult { exit_code: 127, stdout: String::new(), stderr: error_msg }
        }
        Err(e) => {
            let mut error_msg = format!("Unexpected error running {program}: {e}\n");
            if !context.is_empty() {
                error_msg.push_str(&format!("Context: {context}\n"));
            }
            SubprocessResult { exit_code: 1, stdout: String::new(), stderr: error_msg }
        }
    }
}
```

**Key Points**:

- Standard exit codes (127 for command not found)
- Context parameter is critical - always tell users what operation failed
- User-friendly messages with actionable guidance
- No exceptions propagate

### Pattern: Fail-Fast Prerequisite Checking

**Challenge**: Users start using a tool, get cryptic errors mid-workflow when dependencies are missing.

**Solution**: Check all prerequisites at startup with clear, actionable error messages.

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug)]
struct ToolCheckResult {
    tool: String,
    available: bool,
    path: Option<PathBuf>,
    version: Option<String>,
    error: Option<String>,
}

struct PrerequisiteChecker {
    required_tools: HashMap<String, String>,
}

impl PrerequisiteChecker {
    fn new() -> Self {
        let mut required_tools = HashMap::new();
        required_tools.insert("node".into(), "--version".into());
        required_tools.insert("npm".into(), "--version".into());
        required_tools.insert("uv".into(), "--version".into());
        Self { required_tools }
    }

    fn check_and_report(&self) -> bool {
        /// Check prerequisites and print report if any are missing.
        let result = self.check_all_prerequisites();

        if result.iter().all(|r| r.available) {
            return true;
        }

        let missing: Vec<_> = result.iter().filter(|r| !r.available).collect();
        self.format_missing_prerequisites(&missing);
        false
    }
}

struct Launcher;

impl Launcher {
    fn prepare_launch(&self) -> bool {
        /// Check prerequisites FIRST before any other operations
        let checker = PrerequisiteChecker::new();
        if !checker.check_and_report() {
            return false;
        }
        self.setup_environment()
    }
}
```

**Key Points**:

- Check at entry point before any operations
- Check all at once - show all issues
- Structured results with derive structs
- Never auto-install - user control first

### Pattern: Resilient Batch Processing

**Challenge**: Processing large batches where individual items might fail.

**Solution**:

```rust
use chrono::Utc;

struct ResilientProcessor;

#[derive(Debug)]
struct BatchResults<T> {
    succeeded: Vec<T>,
    failed: Vec<FailedItem>,
}

#[derive(Debug)]
struct FailedItem {
    item: String,
    error: String,
    timestamp: String,
}

impl ResilientProcessor {
    async fn process_batch<T: ToString + Clone>(
        &self,
        items: &[T],
    ) -> BatchResults<String> {
        let mut results = BatchResults {
            succeeded: Vec::new(),
            failed: Vec::new(),
        };

        for item in items {
            match self.process_item(item).await {
                Ok(result) => {
                    results.succeeded.push(result);
                    self.save_results(&results); // Save after every item
                }
                Err(e) => {
                    results.failed.push(FailedItem {
                        item: item.to_string(),
                        error: e.to_string(),
                        timestamp: Utc::now().to_rfc3339(),
                    });
                    continue; // Continue processing other items
                }
            }
        }

        results
    }
}
```

**Key Points**:

- Save after every item - never lose progress
- Continue on failure - don't let one failure stop the batch
- Track failure reasons

## Testing & Validation Patterns

### Pattern: TDD Testing Pyramid for System Utilities

**Challenge**: Testing system utilities that interact with external tools while maintaining fast execution.

**Solution**: Follow testing pyramid with 60% unit tests, 30% integration tests, 10% E2E tests.

```rust
/// Tests for module - TDD approach.
///
/// Testing pyramid:
/// - 60% Unit tests (fast, heavily mocked)
/// - 30% Integration tests (multiple components)
/// - 10% E2E tests (complete workflows)

#[cfg(test)]
mod tests {
    use super::*;

    // UNIT TESTS (60%)
    #[test]
    fn test_detect_macos() {
        // With a mock platform detection returning "Darwin"
        let checker = PrerequisiteChecker::with_platform(Platform::MacOS);
        assert_eq!(checker.platform, Platform::MacOS);
    }

    // INTEGRATION TESTS (30%)
    #[test]
    fn test_full_check_workflow() {
        let checker = PrerequisiteChecker::new();
        // With all tools available on PATH
        let result = checker.check_all_prerequisites();
        assert!(result.iter().all(|r| r.available));
    }

    // E2E TESTS (10%)
    #[test]
    fn test_complete_workflow_with_guidance() {
        let checker = PrerequisiteChecker::new();
        let result = checker.check_all_prerequisites();
        let missing: Vec<_> = result.iter().filter(|r| !r.available).collect();
        let message = checker.format_missing_prerequisites(&missing);
        assert!(message.to_lowercase().contains("prerequisite"));
    }
}
```

**Key Points**:

- 60% unit tests for speed
- Strategic mocking of external dependencies
- E2E tests for complete workflows
- All tests run in seconds

## Environment & Platform Patterns

### Pattern: Platform-Specific Installation Guidance

**Challenge**: Users on different platforms need different installation commands.

**Solution**: Detect platform automatically and provide exact installation commands.

```rust
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
enum Platform {
    MacOS,
    Linux,
    Wsl,
    Windows,
}

impl PrerequisiteChecker {
    fn install_commands() -> HashMap<Platform, HashMap<&'static str, &'static str>> {
        let mut commands = HashMap::new();
        commands.insert(Platform::MacOS, HashMap::from([
            ("node", "brew install node"),
            ("git", "brew install git"),
        ]));
        commands.insert(Platform::Linux, HashMap::from([
            ("node", "# Ubuntu/Debian:\nsudo apt install nodejs\n# Fedora:\nsudo dnf install nodejs"),
        ]));
        commands
    }

    fn get_install_command(&self, tool: &str) -> String {
        let commands = Self::install_commands();
        commands
            .get(&self.platform)
            .and_then(|cmds| cmds.get(tool))
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Please install {tool} manually"))
    }
}
```

**Key Points**:

- Automatic platform detection (including WSL)
- Multiple package managers for Linux
- Documentation links for complex installations

### Pattern: Graceful Environment Adaptation

**Challenge**: Different behavior needed in different environments (UVX, normal, testing).

**Solution**: Detect environment automatically and adapt through configuration objects.

```rust
use std::collections::HashMap;

struct EnvironmentAdapter;

#[derive(Debug)]
struct EnvConfig {
    use_add_dir: bool,
    timeout_multiplier: f64,
}

impl EnvironmentAdapter {
    fn detect_environment(&self) -> &str {
        if self.is_uvx_environment() {
            "uvx"
        } else if self.is_testing_environment() {
            "testing"
        } else {
            "normal"
        }
    }

    fn get_config(&self) -> EnvConfig {
        let env = self.detect_environment();
        let config = match env {
            "uvx" => EnvConfig { use_add_dir: true, timeout_multiplier: 1.5 },
            "testing" => EnvConfig { use_add_dir: false, timeout_multiplier: 0.5 },
            _ => EnvConfig { use_add_dir: false, timeout_multiplier: 1.0 },
        };
        self.apply_env_overrides(config) // Allow env variable overrides
    }
}
```

**Key Points**:

- Automatic environment detection
- Configuration objects over scattered conditionals
- Environment variable overrides for customization

## Performance & Optimization Patterns

### Pattern: Intelligent Caching with Lifecycle Management

**Challenge**: Expensive operations repeated unnecessarily, but naive caching leads to memory leaks.

**Solution**: Smart caching with invalidation strategies.

```rust
use std::collections::HashMap;
use std::sync::Mutex;

struct SmartCache {
    cache: Mutex<HashMap<String, String>>,
    hits: Mutex<u64>,
    misses: Mutex<u64>,
}

impl SmartCache {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            hits: Mutex::new(0),
            misses: Mutex::new(0),
        }
    }

    fn expensive_operation(&self, input_data: &str) -> String {
        let cache = self.cache.lock().unwrap();
        if let Some(cached) = cache.get(input_data) {
            *self.hits.lock().unwrap() += 1;
            return cached.clone();
        }
        drop(cache);

        *self.misses.lock().unwrap() += 1;
        let result = self.compute_expensive_result(input_data);
        self.cache.lock().unwrap().insert(input_data.to_string(), result.clone());
        result
    }

    fn invalidate_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    fn get_cache_stats(&self) -> HashMap<String, f64> {
        let hits = *self.hits.lock().unwrap();
        let misses = *self.misses.lock().unwrap();
        let total = hits + misses;
        HashMap::from([
            ("hits".into(), hits as f64),
            ("misses".into(), misses as f64),
            ("hit_rate".into(), hits as f64 / total.max(1) as f64),
        ])
    }
}
```

**Key Points**:

- HashMap with Mutex for thread-safe caching with size control
- Thread safety is essential
- Provide invalidation methods
- Track cache performance

## File I/O & Async Patterns

### Pattern: File I/O with Cloud Sync Resilience

**Challenge**: File operations fail mysteriously when directories are synced with cloud services.

**Solution**:

```rust
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

fn write_with_retry(filepath: &Path, data: &str, max_retries: u32) -> std::io::Result<()> {
    /// Write file with exponential backoff for cloud sync issues
    let mut retry_delay = Duration::from_millis(100);

    for attempt in 0..max_retries {
        if let Some(parent) = filepath.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::write(filepath, data) {
            Ok(()) => return Ok(()),
            Err(e) if e.raw_os_error() == Some(5) && attempt < max_retries - 1 => {
                if attempt == 0 {
                    eprintln!("File I/O error - retrying. May be cloud sync issue.");
                }
                thread::sleep(retry_delay);
                retry_delay *= 2;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
```

**Key Points**:

- Exponential backoff for cloud sync
- Inform user about delays
- Create parent directories

### Pattern: System Metadata vs User Content Classification in Git Operations

**Challenge**: Git-aware operations treat framework-generated metadata files (like `.version`, `.state`) as user content, causing false conflict warnings when files are auto-updated by the system.

**Solution**: Explicitly categorize and filter system-generated files based on semantic purpose, not just directory structure.

```rust
use std::collections::HashSet;

struct GitAwareFileFilter;

impl GitAwareFileFilter {
    /// Distinguish system metadata from user content in git operations

    // System-generated files that should never trigger conflicts
    fn system_metadata() -> HashSet<&'static str> {
        HashSet::from([
            ".version",           // Framework version tracking
            ".state",             // Runtime state
            "settings.json",      // Auto-generated settings
            // Rust build artifacts are in target/ (excluded via .gitignore)
        ])
    }

    fn filter_conflicts(
        &self,
        uncommitted_files: &[String],
        essential_dirs: &[String],
    ) -> Vec<String> {
        /// Filter git status to exclude system metadata
        let metadata = Self::system_metadata();
        let mut conflicts = Vec::new();

        for file_path in uncommitted_files {
            if let Some(relative_path) = file_path.strip_prefix(".claude/") {
                // Skip system-generated metadata - safe to overwrite
                if metadata.contains(relative_path) {
                    continue;
                }

                // Check if file is in essential directories (user content)
                for essential_dir in essential_dirs {
                    if relative_path.starts_with(&format!("{essential_dir}/"))
                        || relative_path == essential_dir
                    {
                        conflicts.push(file_path.clone());
                        break;
                    }
                }
            }
        }

        conflicts
    }
}
```

**Usage in conflict detection**:

```rust
use std::path::Path;
use std::process::Command;

struct ConflictChecker;

#[derive(Debug)]
struct ConflictError(String);

impl ConflictChecker {
    fn check_conflicts(
        &self,
        source_dir: &Path,
        essential_dirs: &[String],
    ) -> Result<(), ConflictError> {
        /// Check for REAL conflicts - ignore system metadata
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(source_dir)
            .output()
            .map_err(|e| ConflictError(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let uncommitted = self.parse_git_status(&stdout);
        let filter = GitAwareFileFilter;
        let user_changes = filter.filter_conflicts(&uncommitted, essential_dirs);

        if !user_changes.is_empty() {
            return Err(ConflictError(format!(
                "Uncommitted user content: {user_changes:?}\n\
                 (System metadata changes are normal and ignored)"
            )));
        }

        Ok(())
    }
}
```

**Key Points**:

- **Semantic categorization**: Filter by PURPOSE (system vs user), not location
- **Root-level awareness**: Don't assume all root files are user content
- **Clear error messages**: Tell users when conflicts are real vs system noise
- **Philosophy alignment**: Ruthlessly simple - add explicit exclusion list
- **Common pitfall**: Only checking subdirectories and missing root-level system files

> **Origin**: Discovered investigating `.version` file causing false conflicts during UVX deployment. See DISCOVERIES.md (2025-12-01).

### Pattern: Async Context Management

**Challenge**: Nested asyncio event loops cause hangs.

**Solution**: Design APIs to be fully async or fully sync, not both.

```rust
// WRONG - Blocks the async runtime
struct Service;

impl Service {
    fn process(&self, data: &str) -> String {
        // Using block_on inside async code causes deadlocks
        tokio::runtime::Runtime::new().unwrap()
            .block_on(self.async_process(data))
    }
}

// RIGHT - Pure async throughout
struct Service;

impl Service {
    async fn process(&self, data: &str) -> String {
        self.async_process(data).await  // No runtime nesting
    }
}
```

**Key Points**:

- Never mix sync/async APIs
- Avoid `block_on()` in libraries
- Let the caller manage the tokio runtime

## Documentation & Investigation Patterns

### Pattern: Documentation Discovery Before Code Analysis

**Challenge**: Agents dive into code without checking if documentation already explains the system.

**Solution**: Always perform documentation discovery before code analysis.

**Process**:

1. Search for documentation files (README, ARCHITECTURE, docs/)
2. Filter by relevance using keywords
3. Read top 5 most relevant files
4. Establish documentation baseline
5. Use docs to guide code analysis

```markdown
Before analyzing [TOPIC], discover existing documentation:

1. Glob: **/README.md, **/ARCHITECTURE.md, **/docs/**/\*.md
2. Grep: Search for keywords related to TOPIC
3. Read: Top 5 most relevant files
4. Establish: What docs claim vs what exists
5. Analyze: Verify code matches docs, identify gaps
```

**Key Points**:

- Always discover docs first (30-second limit)
- Identify doc/code discrepancies
- Graceful degradation for missing docs

## Decision-Making Patterns

### Pattern: Cross-Domain Pattern Applicability Analysis

**Challenge**: Teams import "industry best practices" from other domains without validating applicability, leading to unnecessary complexity.

**Solution**: Five-phase framework for evaluating pattern adoption from other domains.

**Phase 1: Threat Model Match**

- Identify actual failure modes in YOUR system
- Identify pattern's target failure modes
- Verify failure modes match
- If mismatch, REJECT pattern

**Phase 2: Mechanism Appropriateness**

- Does pattern assume adversarial nodes? (Usually wrong for AI agents)
- Does pattern optimize for network communication? (Usually irrelevant for AI)
- Does pattern solve YOUR domain's specific problem?

**Phase 3: Complexity Justification**

```
Justified Complexity: Benefit Gain / Complexity Cost > 3.0
```

If ratio < 3.0, seek simpler alternatives.

**Phase 4: Domain Validation**

- Research pattern's origin domain
- Verify target domain shares those characteristics
- Check for successful applications in similar contexts

**Phase 5: Alternative Exploration**

- Can simpler mechanisms achieve same benefits?
- Can you get 80% of benefit with 20% of complexity?

**Key Points**:

- Threat model mismatch is primary source of inappropriate pattern adoption
- Distributed systems patterns rarely map to AI agent systems
- "Industry best practice" without context validation is a red flag
- Default to ruthless simplicity unless complexity clearly justified

> **Origin**: Discovered evaluating PBZFT vs N-Version Programming. PBZFT would be 6-9x more complex with zero benefit. See DISCOVERIES.md (2025-10-20).

## Multi-Model AI Patterns

### Pattern: Multi-Model Validation Anti-Pattern (STOP Gates)

**Challenge**: Validation checkpoints in AI guidance can trigger model-specific responses, helping one model while breaking another.

**Problem**: STOP gates added to improve Opus caused Sonnet degradation:

- Opus 4.5: STOP gates help (20/22 → 22/22 steps) ✅
- Sonnet 4.5: STOP gates break (22/22 → 8/22 steps) ❌
- Same text, opposite outcomes

**Solution**: Remove validation checkpoints, use flow language instead.

**Example - Bad (STOP Gates)**:

```markdown
## Step 1: Create GitHub Issue

Create an issue for your feature.

## STOP - Verify Issue Created

Before proceeding to Step 2, confirm:

- [ ] GitHub issue created
- [ ] Issue number recorded

Only proceed after verification complete.

## Step 2: Create Branch

...
```

**Example - Good (Flow Language)**:

```markdown
## Step 1: Create GitHub Issue

Create an issue for your feature.

## Step 2: Create Branch

After creating the issue, create a feature branch...
```

**Why This Works**:

- Provides clear structure without interruption points
- Uses flow language ("After X, do Y") not interruption language ("STOP before Y")
- Allows continuous autonomous execution
- Works for both models

**Empirical Evidence** (Issue #1755, 6/8 benchmarks complete):

| Model  | With STOP Gates  | Without STOP Gates (V2)           |
| ------ | ---------------- | --------------------------------- |
| Sonnet | 8/22 steps (36%) | 22/22 steps (100%)                |
| Opus   | 22/22 steps      | ~20/22 steps (maintains baseline) |

**Performance Results**:

- Sonnet V2: -16% cost improvement
- Opus V2: -21% cost improvement
- Removing gates IMPROVES performance (STOP Gate Paradox)

**Key Points**:

- Different models interpret "STOP" differently
- Opus: Treats as checkpoint, proceeds
- Sonnet: Treats as permission gate, asks user
- High-salience language ("STOP", "MUST", ALL CAPS) risky
- Always test multi-model before deploying guidance changes

**When to Use Flow Language**:

- "After X, proceed to Y" ✅
- "When X completes, Y begins" ✅
- "Following X, continue with Y" ✅

**When to AVOID Interruption Language**:

- "STOP before Y" ❌
- "Only proceed after X" ❌
- "Wait for confirmation before Y" ❌

**Related**: Issue #1755, DISCOVERIES.md (2025-12-01)
**Validation**: 75% complete (6/8 benchmarks), both models tested
**Impact**: $20K-$406K annual savings from removing STOP gates

---

## Multi-Model AI Patterns

### Pattern: AI-Optimized Workflows (No Human Psychology)

> **Philosophy Reference**: See @~/.amplihack/.claude/context/PHILOSOPHY.md "Ruthless Simplicity" and "Code you don't write has no bugs"

**Challenge**: Workflows designed with human psychology (commitment, celebration) add overhead for AI agents without providing benefit.

**Solution**: Remove psychological framing, keep only essential workflow steps.

```markdown
# ANTI-PATTERN - Human Psychology in AI Workflows ❌

## Workflow Contract

By reading this workflow file, you are committing to execute ALL 22 steps.
**Your Commitment**: [commitment checkboxes]

[22 Workflow Steps]

## 🎉 Workflow Complete!

Congratulations! You executed all 22 steps systematically.
[Celebration and verification]

# GOOD PATTERN - AI-Optimized Workflow ✅

[22 Workflow Steps - Just the steps, no psychology]
```

**Empirical Evidence** (V8 Testing, Issue #1785):
| Metric | With Psychology | Without Psychology | Improvement |
|--------|----------------|-------------------|-------------|
| Cost (MEDIUM) | Unknown | $2.93-$8.36 (avg $5.62) | 72-95% |
| Cost (HIGH) | Unknown | $13.56-$31.95 (avg $21.72) | Est. 90% |
| Quality | Unknown | 100% (22/22) | 100% |
| Lines | 482 | 443 | -8% |

**Key Points**:

- AI agents don't need commitment (already committed by design)
- AI agents don't experience celebration (wasted tokens)
- Psychological framing = ~8% overhead with zero benefit
- Removal improves performance 72-95% while maintaining 100% quality
- Builder autonomously applied this pattern (removed psychology without being told)

**When to Use**:

- Designing workflows for AI agents
- Optimizing prompts for AI consumption
- Creating AI-facing documentation
- Any content primarily read by AI (not humans)

**When NOT to Use**:

- Human-facing documentation (humans benefit from psychology)
- User-facing guides (motivation helps users)
- Team communication (celebration builds culture)

**Philosophy Alignment**:

- ✅ Ruthless simplicity (remove non-essential)
- ✅ "Code you don't write has no bugs" (applied to prompts)
- ✅ Minimize abstractions (removed psychological layer)
- ✅ Essential only (Wabi-sabi)

> **Origin**: V8 testing (Issue #1785, 2025-12-02). Builder agent autonomously removed psychological framing, achieving 90% cost reduction. See tag: v8-no-psych-winner, Archive: .claude/runtime/benchmarks/v8_experiments_archive_20251202_212646/

---

## Remember

These patterns represent proven solutions from real development challenges:

1. **Check this document first** - Don't reinvent solutions
2. **Update when learning** - Keep patterns current
3. **Include context** - Explain why, not just how
4. **Show working code** - Examples should be copy-pasteable
5. **Document gotchas** - Save others from the same pain
