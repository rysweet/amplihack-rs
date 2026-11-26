# qa-team: parity and shadow

## Side-by-Side Parity and A/B Validation [LEVEL 2]

Use QA Team for standard outside-in scenarios **and** for parity loops where you must compare a legacy implementation to a replacement, or compare approach A to approach B, as an external user would observe them.

### Use QA Team for parity work when

- migrating Python to Rust, old CLI to new CLI, or v1 to v2 behavior
- validating a rewrite before switching defaults
- comparing branch A vs branch B using the same user scenarios
- running observable side-by-side sessions in paired virtual TTYs
- logging rollout divergences in shadow mode without failing the run

### Recommended parity loop

1. Define shared user-facing scenarios first.
2. Run both implementations in isolated sandboxes.
3. Compare stdout, stderr, exit code, JSON outputs, and filesystem side effects.
4. Re-run in `--observable` mode when you need paired tmux panes for debugging.
5. Use `--ssh-target <host>` when parity must happen on a remote environment such as `azlin`.
6. Use `--shadow-mode --shadow-log <file>` during rollout to log divergences without blocking execution.

**tmux in tests and sandboxes:** pass `-S <private socket>` (or use `env -u TMUX tmux -S <private socket>`) on every tmux command. When `$TMUX` is set, as it is inside any tmux pane, tmux uses that socket and ignores `TMUX_TMPDIR`, so `TMUX_TMPDIR=… tmux kill-server` kills the user's own server. Never run `kill-server` or `kill-session -a` without `-S`/`-L`; a PreToolUse hook denies them.

### Command pattern to reuse

For Rust CLI repos, extend native integration tests instead of reintroducing a
scripted parity harness. A good baseline is:

```bash
cargo test -p amplihack --test integration cli_launch:: --locked
cargo test -p amplihack --test integration no_python_probe:: --locked
```

For repository no-Python validation:

```bash
scripts/check-no-python-assets.sh
scripts/probe-no-python.sh
```

For rollout shadow logging, prefer the product's native JSON/logging modes and
compare artifacts with shell or Rust test helpers:

```bash
cargo test --workspace --locked
```

## Level 4: Shadow Environment Integration [LEVEL 4]

Run your outside-in tests in **isolated shadow environments** to validate changes before pushing. This combines the behavioral testing power of gadugi-agentic-test with the clean-state isolation of shadow environments.

### Why Use Shadow Environments for Testing

1. **Clean State**: Fresh container, no host pollution
2. **Local Changes**: Test uncommitted code exactly as-is
3. **Multi-Repo**: Coordinate changes across multiple repos
4. **CI Parity**: What shadow sees ≈ what CI will see

### Shadow Testing Workflow

For complete shadow environment documentation, see the **shadow-testing** skill. Here's how to integrate it with outside-in tests:

#### Pattern 1: CLI Tests in Shadow (Amplifier)

```python
# Create shadow with your local library changes
shadow.create(local_sources=["~/repos/my-lib:org/my-lib"])

# Run outside-in test scenarios inside shadow
shadow.exec(shadow_id, "gadugi-test run test-scenario.yaml")

# Extract evidence
shadow.extract(shadow_id, "/evidence", "./test-evidence")

# Cleanup
shadow.destroy(shadow_id)
```

#### Pattern 2: CLI Tests in Shadow (Standalone)

```bash
# Create shadow with local changes
amplifier-shadow create --local ~/repos/my-lib:org/my-lib --name test

# Run your test scenarios
amplifier-shadow exec test "gadugi-test run test-scenario.yaml"

# Extract results
amplifier-shadow extract test /evidence ./test-evidence

# Cleanup
amplifier-shadow destroy test
```

#### Pattern 3: Multi-Repo Integration Test

```yaml
# test-multi-repo.yaml
scenario:
  name: "Multi-Repo Integration Test"
  type: cli

  prerequisites:
    - "Shadow environment with core-lib and cli-tool"

  steps:
    - action: launch
      target: "cli-tool"

    - action: send_input
      value: "process --lib core-lib\n"

    - action: verify_output
      contains: "Success: Using core-lib"
```

```bash
# Setup shadow with both repos
amplifier-shadow create \
  --local ~/repos/core-lib:org/core-lib \
  --local ~/repos/cli-tool:org/cli-tool \
  --name multi-test

# Run test that exercises both
amplifier-shadow exec multi-test "gadugi-test run test-multi-repo.yaml"
```

#### Pattern 4: Web App Testing in Shadow

```yaml
# test-web-app.yaml
scenario:
  name: "Web App with Local Library"
  type: web

  steps:
    - action: navigate
      url: "http://localhost:3000"

    - action: click
      selector: "button.process"

    - action: verify_element
      selector: ".result"
      contains: "Processed with v2.0" # Your local version
```

```bash
# Shadow with library changes
amplifier-shadow create --local ~/repos/my-lib:org/my-lib --name web-test

# Start web app inside shadow (uses your local lib)
amplifier-shadow exec web-test "
  cd /workspace &&
  git clone https://github.com/org/web-app &&
  cd web-app &&
  npm install &&  # Pulls your local my-lib via git URL rewriting
  npm start &
"

# Wait for app to start, then run tests
amplifier-shadow exec web-test "sleep 5 && gadugi-test run test-web-app.yaml"
```

### Verification Best Practices

When running tests in shadow, always verify your local sources are being used:

```bash
# After shadow.create, check snapshot commits
shadow.status(shadow_id)
# Shows: snapshot_commits: {"org/my-lib": "abc1234..."}

# When your test installs dependencies, verify commit matches
# Look in test output for: my-lib @ git+...@abc1234
```

### Complete Example: Library Change Validation

```yaml
# test-library-change.yaml - Outside-in test
scenario:
  name: "Validate Library Breaking Change"
  type: cli
  description: "Test that dependent app still works with new library API"

  steps:
    - action: launch
      target: "/workspace/org/dependent-app/cli.py"

    - action: send_input
      value: "process data.json\n"

    - action: verify_output
      contains: "Processed successfully"
      description: "New library API should still work"

    - action: verify_exit_code
      expected: 0
```

```bash
# Complete workflow
# 1. Create shadow with your breaking change
amplifier-shadow create --local ~/repos/my-lib:org/my-lib --name breaking-test

# 2. Install dependent app (pulls your local lib)
amplifier-shadow exec breaking-test "
  cd /workspace &&
  git clone https://github.com/org/dependent-app &&
  cd dependent-app &&
  pip install -e . &&  # This installs git+https://github.com/org/my-lib (your local version)
  echo 'Ready to test'
"

# 3. Run outside-in test
amplifier-shadow exec breaking-test "gadugi-test run test-library-change.yaml"

# If test passes, your breaking change is compatible!
# If test fails, you've caught the issue before pushing
```

### When to Use Shadow Integration

Use shadow + outside-in tests when:

- ✅ Testing library changes with dependent projects
- ✅ Validating multi-repo coordinated changes
- ✅ Need clean-state validation before pushing
- ✅ Want to catch integration issues early
- ✅ Testing that setup/install procedures work

Don't use shadow for:

- ❌ Simple unit tests (too much overhead)
- ❌ Tests of already-committed code (shadow adds no value)
- ❌ Performance testing (container overhead skews results)

### Learn More

For complete shadow environment documentation, including:

- Shell scripts for DIY setup
- Docker Compose examples
- Multi-language support (Python, Node, Rust, Go)
- Troubleshooting and verification techniques

**Load the shadow-testing skill**:

```
Claude, use the shadow-testing skill to set up a shadow environment
```

Or for Amplifier users, the shadow tool is built-in:

```python
shadow.create(local_sources=["~/repos/lib:org/lib"])
```

---

### Related Skills

- **shadow-testing**: Complete shadow environment setup and usage
- **test-gap-analyzer**: Find untested code paths
- **philosophy-guardian**: Review test philosophy compliance
- **reviewing-code**: Include tests in PR reviews
- **module-spec-generator**: Generate specs with test scenarios

### Further Reading

- Outside-in vs inside-out testing approaches
- Behavior-driven development (BDD) principles
- AI-powered testing best practices
- Test automation patterns
- Shadow environment testing methodology

## Level 4: A/B Comparison Harness & Self-Improving Audit [LEVEL 4]

Reusable tools for running any two CLI implementations side-by-side and
quantifying their behavioral differences. Works for any scenario where you
need to compare two things:

- **Migration parity**: Python→Rust, v1→v2, monolith→microservice
- **A/B testing**: Compare two approaches to the same CLI
- **Feature flag validation**: Same binary, different configs
- **Version regression**: Compare release N with release N+1
- **Canary validation**: Compare canary build against stable

### Tools

Located in `.claude/scenarios/ab-comparison/`:

| File                       | Purpose                                                        |
| -------------------------- | -------------------------------------------------------------- |
| `ab-comparison-harness`    | Run two CLIs side-by-side in isolated sandboxes                |
| `ab-audit-cycle`           | Self-improving loop: identify → categorize → fix → re-validate |
| `SCENARIO_FORMAT.md`       | YAML scenario format specification                             |
| `example_scenario.yaml`    | Example scenario for a calculator CLI                          |

### Quick Start

```bash
# Compare any two CLI implementations
ab-comparison-harness \
    --a "myapp-stable" \
    --b "./target/debug/myapp" \
    --scenario tests/scenarios/*.yaml

# A/B with feature flag
ab-comparison-harness \
    --a "myapp --feature-flag=off" \
    --b "myapp --feature-flag=on" \
    --scenario tests/scenarios/regression.yaml

# Shadow mode: log divergences without failing (for production rollout)
ab-comparison-harness \
    --a "myapp-stable" \
    --b "myapp-canary" \
    --scenario tests/scenarios/*.yaml \
    --shadow-mode \
    --shadow-log /tmp/canary-divergences.jsonl

# Self-improving audit cycle (iterate until convergence)
ab-audit-cycle \
    --a "myapp-stable" \
    --b "./target/debug/myapp" \
    --scenarios-dir tests/scenarios/

# Legacy flags (--legacy/--candidate) also work
ab-comparison-harness \
    --legacy "myapp-stable" \
    --candidate "./target/debug/myapp" \
    --scenario tests/scenarios/*.yaml
```

### Scenario Format (Summary)

```yaml
cases:
  - name: "unique-test-name"
    category: "install" # For audit grouping
    argv: ["subcommand", "--flag"] # Same args for both sides
    timeout: 15 # Seconds
    env:
      PATH: "${SANDBOX_ROOT}/bin:${PATH}" # Template variables
    setup: | # Bash script run in both sandboxes
      mkdir -p bin
      echo '#!/bin/bash' > bin/stub && chmod +x bin/stub
    compare:
      - exit_code # Integer match
      - stdout # Text or JSON-semantic
      - stderr # Text match
      - "fs:path/to/file" # File/dir hash comparison
      - "jsonfs:path/to/file.json" # JSON semantic comparison
```

See `SCENARIO_FORMAT.md` for the full specification.

### Key Design Decisions

1. **Isolated sandboxes**: Each side gets its own HOME, TMPDIR, PATH. No
   collisions between the two runs, or with the host.

2. **JSON-semantic comparison**: `stdout` comparison parses both sides as JSON
   if possible, ignoring key ordering. Falls back to exact text if not JSON.

3. **Stub binary pattern**: Test launcher behavior by providing fake binaries
   that capture their args and env vars to files, then compare those files.

4. **Timeout resilience**: Captures partial output on timeout (exit code 124)
   instead of crashing the harness. Critical for testing commands that may
   hang in sandboxed environments.

5. **Shadow mode**: Logs divergences to append-only JSONL without failing the
   run. Use during rollout to monitor real workloads.

6. **Audit categorization**: Groups divergences by keyword inference and
   generates prioritized fix workstream specs.

### Self-Improvement Cycle Pattern

```
┌─────────────┐
│  IDENTIFY   │  Run all scenarios, collect divergences
└──────┬──────┘
       ▼
┌─────────────┐
│ CATEGORIZE  │  Group by area: install (5), launch (3), recipe (1)
└──────┬──────┘
       ▼
┌─────────────┐
│  SPECIFY    │  Generate fix specs: [CRITICAL] fix-install, [HIGH] fix-launch
└──────┬──────┘
       ▼
┌─────────────┐
│    FIX      │  Agent/human fixes code
└──────┬──────┘
       ▼
┌─────────────┐
│ RE-VALIDATE │  Run audit again → check progress (5→2 divergences)
└──────┬──────┘
       ▼
    Converged? ──no──→ back to FIX
       │
      yes
       ▼
     DONE
```

### Real-World Results

Used for `amplihack-rs` migration validation (rysweet/amplihack-rs#25):

- **118/118** comparison tests across 12 tiers
- **619/619** hook golden file tests
- **9/9** shadow harness cases
- 5 Python bugs discovered and fixed
- 1 recipe orchestrator bug discovered and fixed
- Full launcher behavioral match achieved
