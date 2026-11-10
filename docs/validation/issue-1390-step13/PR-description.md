## Step 13: Local Testing Results

Detected toolchains:
- Rust/Cargo: workspace Cargo.toml, Cargo.lock, bins/amplihack/src/main.rs, changed CLI integration test. README documents cargo test --workspace --locked.

Chosen validation strategy:
- cargo build --locked -p amplihack --bin amplihack, with CARGO_TARGET_DIR=/private/tmp/issue-1390-target; exit 0.
- Retained standalone Rust Cargo harness invokes that current CLI with deterministic Rust runner fixtures. It adapts the current issue regression, preserving its assertions and shared isolation helper. This avoids the consolidated integration target's previously reported unrelated macOS proc_link compilation blocker without disabling repository tests.
- Temporary HOME, profiles, configuration and fake process table; explicit AMPLIHACK_AGENT_BINARY=claude. No quota probes or timed behavioral assertions. Process completion synchronizes assertions.
- An exploratory run exposed a read of real ~/.claude/runtime/launcher_context.json despite temporary HOME. That run violated isolation and is excluded as passing evidence. Both retained runs explicitly select the agent and report agent_binary_source=env.
- Classification and evidence preservation are tested. Session pausing and transcript recovery are not claimed; the maintainer's issue comment assigns runtime lifecycle recovery upstream.

### Scenario 1 — Session limit survives the CLI boundary
Command: `GIT_TERMINAL_PROMPT=0 CARGO_TARGET_DIR=/private/tmp/issue-1390-target timeout --foreground --kill-after=60 1200 cargo test --offline --locked --manifest-path /private/tmp/issue-1390-step13/harness/Cargo.toml simple:: -- --nocapture`
Result: PASS
Output: CLI exit=Some(1); class usage_limit; signal session limit; reset hint 9:20am (UTC); test result: ok. 1 passed; 0 failed.
Assertions verify matching JSON/stderr classification, terminal action, retryable false, attempt 1, failed step ID, and exactly one runner invocation.
Full output: simple.log. Numeric test exit receipt: simple.exit (0). Expected CLI failure exit 1 is asserted.

### Scenario 2 — HTTP 429 envelope preserves completed work
Command: `GIT_TERMINAL_PROMPT=0 CARGO_TARGET_DIR=/private/tmp/issue-1390-target timeout --foreground --kill-after=60 1200 cargo test --offline --locked --manifest-path /private/tmp/issue-1390-step13/harness/Cargo.toml complex:: -- --nocapture`
Result: PASS
Output: CLI exit=Some(1); class usage_limit; completed_steps contains prepare; reset hint retained; test result: ok. 1 passed; 0 failed.
Usage-limit classification wins over the HTTP 429 marker. Completed/failed step IDs, matching diagnostics and exactly one invocation are asserted.
Full output: complex.log. Numeric test exit receipt: complex.exit (0).

Mac lane limitations:
- Linux-only /proc, pidfd and FIFO-pool tests did not run here. The registered cli_integration regression requires Linux CI because of the unrelated consolidated-target import blocker. Standalone passes do not claim workspace success.
- Existing Ubuntu CI Test job runs cargo nextest run --workspace --locked. No CI checks added.
- Harness source and lockfile are retained. The wrapper records absolute paths to this checkout and the local binary. Source SHA is in source.txt.
