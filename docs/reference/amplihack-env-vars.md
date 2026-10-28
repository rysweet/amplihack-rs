# AMPLIHACK_* Environment Variable Audit

Every distinct `AMPLIHACK_*` name that appears in `crates/` and `bins/`, with
what it is for and, where one exists, the
[user config](./user-config.md) key that sets the same thing. Use this table to
decide whether a variable is yours to set.

## How this table is generated

The name list comes from this command, run at the repository root:

```sh
git grep -ohE 'AMPLIHACK_[A-Z0-9_]*[A-Z0-9]' -- crates bins | sort -u
```

Every name it prints has exactly one row below. When code adds or removes a
name, rerun the command and update the table in the same change.

The grep also finds identifiers that are not environment variables: Rust
constants, marker strings, heredoc delimiters, and prefixes of a variable
family named in help text. They are listed so the table stays complete; their
note starts with "Not an env var".

## Categories

| Category | Meaning |
|----------|---------|
| user preference | You may set it to change amplihack's behaviour. It stays supported. |
| internal | amplihack sets it for its own children, or it is not a variable at all. Do not set it by hand. |
| test-only | Read or set only by tests and test fixtures. It has no effect in normal use. |

A variable with a config key keeps working and outranks the config files. See
[Precedence](./user-config.md#precedence).

## Variables

| Name | Category | Config key | Note |
|------|----------|------------|------|
| `AMPLIHACK_1552_REQUIRE_RECIPE_RUNNER` | test-only |  | Makes the #1552 integration test fail instead of skip when no runner is installed. |
| `AMPLIHACK_1570_REQUIRE_RECIPE_RUNNER` | test-only |  | Same, for the #1570 context-file test. |
| `AMPLIHACK_AGENTS` | internal |  | Not an env var: the hooks crate's static list of known agent names. |
| `AMPLIHACK_AGENT_BINARY` | user preference | `agent.binary` | Agent CLI override; also exported to children with a source tag. |
| `AMPLIHACK_AGENT_BINARY_SOURCE` | internal |  | Tag describing where an exported `AMPLIHACK_AGENT_BINARY` came from (`session:`, `default:`, `tmux_server:`, `user_config:`). |
| `AMPLIHACK_ALLOW` | test-only |  | Not an env var: half of a split string in a runner-ancestry test that proves the unreleased pre-#1593 name no longer forces a downgrade. |
| `AMPLIHACK_AMPLIHACK_BINARY_PATH` | test-only |  | Instance of the `AMPLIHACK_<TOOL>_BINARY_PATH` lookup, set only by blarify tests. |
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | internal |  | `AMPLIHACK_<TOOL>_BINARY_PATH` lookup for `amplihack-hooks` during install. |
| `AMPLIHACK_ARTIFACT_GUARD_BASELINE` | internal |  | Baseline handed to the artifact guard by workflow steps. |
| `AMPLIHACK_ARTIFACT_GUARD_PREEXISTING` | internal |  | Pre-existing paths handed to the artifact guard by workflow steps. |
| `AMPLIHACK_ASSET_RESOLVER` | internal |  | Path to `amplihack-asset-resolver`, exported to recipe steps. |
| `AMPLIHACK_AUTO_MAX_TURNS` | user preference |  | Default `--max-turns` for auto mode (clap `env`). |
| `AMPLIHACK_AUTO_MODE` | internal |  | Marks a process started by auto mode. |
| `AMPLIHACK_AUTO_SWITCH_SUPERVISED` | internal |  | Marks a Claude process supervised by profile auto-switch. |
| `AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER` | test-only |  | Fault injection for the auto-switch swap. |
| `AMPLIHACK_BADTOOL_BINARY_PATH` | test-only |  | `binary_finder` unit-test instance of `AMPLIHACK_<TOOL>_BINARY_PATH`. |
| `AMPLIHACK_BLARIFY_MODE` | user preference |  | Blarify indexing mode. |
| `AMPLIHACK_CLAUDE_AUTO_SWITCH` | user preference |  | Enables Claude profile auto-switch. |
| `AMPLIHACK_CLAUDE_BINARY_PATH` | user preference |  | Path to the `claude` executable. |
| `AMPLIHACK_CLAUDE_PROFILE` | user preference |  | Claude profile to launch with. |
| `AMPLIHACK_CLAUDE_PROFILE_ROOT` | user preference |  | Directory that holds Claude profiles. The test helper `isolated(..)` removes it from every child; see [Keep Tests out of the Real Profile Root](../howto/keep-tests-out-of-the-real-profile-root.md). |
| `AMPLIHACK_CLI_INTEGRATION_QUIET_CHILD` | test-only |  | Test-support child-process switch. |
| `AMPLIHACK_CODEX_BINARY_PATH` | test-only |  | Set only by install tests; the lookup itself is `AMPLIHACK_<TOOL>_BINARY_PATH`. |
| `AMPLIHACK_CODEX_TERMINAL_TEST_FIXTURE` | test-only |  | Launcher test fixture switch. |
| `AMPLIHACK_CONTEXT` | internal |  | Prefix of the `AMPLIHACK_CONTEXT_<KEY>` variables `recipe run` exports for context values. |
| `AMPLIHACK_CONTEXT_ENV_BUDGET_BYTES` | user preference |  | Size budget for context values exported as env vars. |
| `AMPLIHACK_CONTEXT_FILE` | internal |  | Path of the context file `recipe run` hands to the runner. |
| `AMPLIHACK_CONTEXT_START` | internal |  | Not an env var: marker string around injected context. |
| `AMPLIHACK_COPILOT` | test-only |  | Not an env var: fixture string in the #1481 test. |
| `AMPLIHACK_COPILOT_BINARY_PATH` | test-only |  | Set only by prompt-delivery tests; the lookup itself is `AMPLIHACK_<TOOL>_BINARY_PATH`. |
| `AMPLIHACK_COPILOT_NO_ALLOW_ALL` | user preference |  | Stops `amplihack copilot` from passing `--allow-all-*`. |
| `AMPLIHACK_COPILOT_NO_REMOTE` | user preference |  | Stops `amplihack copilot` from enabling remote access. |
| `AMPLIHACK_DATABASE_PASSWORD` | internal |  | Example secret the Docker launcher must not forward; named in its test. |
| `AMPLIHACK_DEBUG` | internal |  | Not read: example name in `amplihack-state` `EnvVar` docs. |
| `AMPLIHACK_DEFAULT_MODEL` | user preference |  | Default model for launched agents. |
| `AMPLIHACK_DELEGATE` | user preference |  | Delegate command for multitask workstreams. |
| `AMPLIHACK_DELIVERY_WORKER` | test-only |  | Re-exec switch for the freshness delivery test worker. |
| `AMPLIHACK_DEPTH` | internal |  | Launch nesting depth exported to children. |
| `AMPLIHACK_DISABLE_BLARIFY` | user preference |  | Disables blarify indexing in the session-start hook. |
| `AMPLIHACK_DOWNGRADE_GUARD_WORKER` | test-only |  | Re-exec switch for the downgrade-guard test worker. |
| `AMPLIHACK_ENABLE_BLARIFY` | user preference |  | Opts in to blarify indexing. |
| `AMPLIHACK_ENABLE_REFLECTION` | user preference |  | Enables the stop-hook reflection. |
| `AMPLIHACK_EXTERNAL_OTLP_AUTHORIZATION` | internal |  | OTLP header the Docker launcher must not forward; named in its filter. |
| `AMPLIHACK_FLEET_EXISTING_VMS` | user preference |  | Existing VMs for `amplihack fleet` to use. |
| `AMPLIHACK_FLEET_REASONER_BINARY_PATH` | user preference |  | Path to the fleet reasoner's CLI. |
| `AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE` | user preference |  | Allows installing an older recipe runner. |
| `AMPLIHACK_GIT_1278_CHILD` | test-only |  | Re-exec switch for the #1278 git test. |
| `AMPLIHACK_GIT_1278_PLAIN` | test-only |  | Re-exec switch for the #1278 git test. |
| `AMPLIHACK_GRAPH_DB_PATH` | internal |  | Code-graph database path exported to children. |
| `AMPLIHACK_HOME` | user preference | `paths.home` | Root for framework assets, hooks and runtime state. |
| `AMPLIHACK_HOOK_CONTRACT` | internal |  | Not an env var: suffix of the `CANONICAL_AMPLIHACK_HOOK_CONTRACT` constant. |
| `AMPLIHACK_HOOK_PROTOCOL` | internal |  | Hook wire protocol, set by the Codex plugin's wrapper script. |
| `AMPLIHACK_HOOK_SPECS` | internal |  | Not an env var: the install crate's hook spec table. |
| `AMPLIHACK_IDLE_POLL_MS` | user preference |  | Idle-watchdog poll interval. |
| `AMPLIHACK_IDLE_TIMEOUT_SECS` | user preference |  | Idle-watchdog timeout. |
| `AMPLIHACK_INDICATORS` | internal |  | Not an env var: the project-init marker list. |
| `AMPLIHACK_INSTRUCTIONS_END` | internal |  | Not an env var: end marker of staged Copilot instructions. |
| `AMPLIHACK_INSTRUCTIONS_START` | internal |  | Not an env var: start marker of staged Copilot instructions. |
| `AMPLIHACK_INVESTIGATION_REPORT_EOF` | test-only |  | Not an env var: heredoc delimiter in the #1552 test. |
| `AMPLIHACK_IN_DOCKER` | internal |  | Marks a process running in amplihack's container. |
| `AMPLIHACK_IN_UVX` | internal |  | Marks a process started through uvx. |
| `AMPLIHACK_ISSUE1570_KEEP` | test-only |  | Keeps the #1570 test's temp files for inspection. |
| `AMPLIHACK_IS_STAGED` | internal |  | Marks an auto-mode launch from a staged framework copy. |
| `AMPLIHACK_KUZU_DB_PATH` | internal |  | Back-compat alias of `AMPLIHACK_GRAPH_DB_PATH`. |
| `AMPLIHACK_LAUNCHER` | internal |  | Launcher name recorded in `launcher_context.json`. |
| `AMPLIHACK_LITELLM` | internal |  | Prefix of the `AMPLIHACK_LITELLM_*` variables, named in help text. |
| `AMPLIHACK_LITELLM_API_KEY` | user preference |  | External LiteLLM gateway key. |
| `AMPLIHACK_LITELLM_ENDPOINT` | user preference |  | External LiteLLM gateway URL. |
| `AMPLIHACK_LITELLM_EXPECTED_GATEWAY_IDENTITY` | user preference |  | Gateway identity the preflight expects. |
| `AMPLIHACK_LITELLM_EXPECTED_MODEL` | user preference |  | Model the preflight expects. |
| `AMPLIHACK_LITELLM_EXPECTED_PROVIDER` | user preference |  | Provider the preflight expects. |
| `AMPLIHACK_LITELLM_MODEL` | user preference |  | Model routed through the gateway. |
| `AMPLIHACK_LITELLM_TARGET` | user preference |  | Agent CLI routed through the gateway. |
| `AMPLIHACK_LITELLM_TELEMETRY_FILE` | user preference |  | Preflight telemetry output file. |
| `AMPLIHACK_LITELLM_TELEMETRY_HMAC_KEY` | user preference |  | Key that signs preflight telemetry. |
| `AMPLIHACK_MAX_DEPTH` | user preference | `orchestrator.max_depth` | Session-tree recursion ceiling for a new root; can only lower a sealed tree. |
| `AMPLIHACK_MAX_LOCK_ITERATIONS` | user preference |  | Lock-mode iteration cap in the stop hook. |
| `AMPLIHACK_MAX_LOG_BYTES` | user preference |  | Multitask log size cap. |
| `AMPLIHACK_MAX_PARALLEL_WORKSTREAMS` | user preference |  | Multitask wave width. |
| `AMPLIHACK_MAX_SESSIONS` | user preference |  | Session-tree session cap. |
| `AMPLIHACK_MEMORY` | internal |  | Prefix of the `AMPLIHACK_MEMORY_*` variables, named in docs. |
| `AMPLIHACK_MEMORY_BACKEND` | user preference |  | Memory backend selection. |
| `AMPLIHACK_MEMORY_ENABLED` | user preference |  | Enables agent memory in the launcher crate. |
| `AMPLIHACK_MEMORY_GOSSIP_ENABLED` | user preference |  | Distributed-memory gossip switch. |
| `AMPLIHACK_MEMORY_QUERY_FANOUT` | user preference |  | Distributed-memory query fan-out. |
| `AMPLIHACK_MEMORY_REPLICATION_FACTOR` | user preference |  | Distributed-memory replication factor. |
| `AMPLIHACK_MEMORY_STORAGE_PATH` | user preference |  | Memory storage path. |
| `AMPLIHACK_MEMORY_TOKEN_BUDGET` | user preference |  | Memory injection token budget. |
| `AMPLIHACK_MEMORY_TOPOLOGY` | user preference |  | Distributed-memory topology. |
| `AMPLIHACK_MEMORY_TRANSPORT` | user preference |  | Distributed-memory transport. |
| `AMPLIHACK_MERGE_GUARD` | internal |  | Set by `recipe run` for auto-drive recipes; the merge guard (`gh pr merge` shim and pre-tool-use hook) is active while it is non-empty. |
| `AMPLIHACK_MERGE_GUARD_USER_BASH_ENV` | internal |  | The user's own `BASH_ENV`, kept while the merge guard's `BASH_ENV` file is in place; that file sources it first. |
| `AMPLIHACK_MIN_AVAILABLE_MIB` | user preference |  | Free-memory floor before a session-tree spawn. |
| `AMPLIHACK_MODE` | user preference |  | Forces the detected framework mode. |
| `AMPLIHACK_NONINTERACTIVE` | internal |  | Marks a delegated, non-interactive process; set by `recipe run` for every step. |
| `AMPLIHACK_NOT_SET_1551` | test-only |  | Not an env var: a name an env-builder unit test checks is absent. |
| `AMPLIHACK_NO_FRESHNESS_CHECK` | user preference |  | Skips the binary freshness check. |
| `AMPLIHACK_NO_RUST_BOOTSTRAP` | user preference |  | Skips the Rust toolchain bootstrap. |
| `AMPLIHACK_NO_SYSTEM_PROMPT_APPEND` | user preference |  | Skips the system-prompt append. |
| `AMPLIHACK_NO_UPDATE_CHECK` | user preference |  | Skips the startup update check. |
| `AMPLIHACK_NPM_SENTINEL` | test-only |  | Sentinel path for the npm update-check test script. |
| `AMPLIHACK_NPM_VERSION` | test-only |  | Not an env var: placeholder in the plugin-manifest test. |
| `AMPLIHACK_ORCHESTRATION_BOOTSTRAP` | test-only |  | Codex front-door contract fixture. |
| `AMPLIHACK_ORIGINAL_CWD` | internal |  | Original directory handed to an auto-mode child. |
| `AMPLIHACK_OTEL_ENABLED` | user preference |  | Enables OpenTelemetry export. |
| `AMPLIHACK_PARITY_TEST` | test-only |  | Marks parity-test runs; skips the update prompt. |
| `AMPLIHACK_PLUGIN_AUTO_INSTALL` | test-only |  | Plugin-manifest test fixture. |
| `AMPLIHACK_PLUGIN_INSTALLED` | internal |  | Set by the Claude Code plugin's bootstrap. |
| `AMPLIHACK_PLUGIN_INSTALL_LOCK` | test-only |  | Plugin-manifest test fixture. |
| `AMPLIHACK_POST_UPDATE_INSTALL` | internal |  | Marks the install that follows a self-update. |
| `AMPLIHACK_PROBE_BIN` | test-only |  | Probe binary path in update and install tests. |
| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | user preference |  | Timeout for `amplihack profile usage`. |
| `AMPLIHACK_PROJECT_ID` | user preference |  | Memory learning project id. |
| `AMPLIHACK_PROJECT_ROOT` | internal |  | Project root exported by the uvx manager. |
| `AMPLIHACK_PROMPT_DELIVERY` | user preference |  | Prompt delivery mode for agent subprocesses. |
| `AMPLIHACK_QUEUE_PATH` | test-only |  | Fleet test fixture. |
| `AMPLIHACK_RECIPE_LOG_JSONL` | user preference |  | Path for structured runner events. |
| `AMPLIHACK_RECIPE_RUNNER_TIMEOUT_SECS` | user preference |  | Whole-run wall-clock limit for the recipe runner. |
| `AMPLIHACK_RECIPE_RUN_ID` | internal |  | Run id `recipe run` hands to the runner. |
| `AMPLIHACK_RECIPE_TRANSIENT_BUDGET_SECS` | user preference |  | Time budget for transient-failure retries. |
| `AMPLIHACK_RECIPE_TRANSIENT_MAX_ATTEMPTS` | user preference |  | Attempt cap for transient-failure retries. |
| `AMPLIHACK_REFLECTION_BINARY` | user preference |  | CLI the reflection hook runs. |
| `AMPLIHACK_RELEASE_VERSION` | internal |  | Release version stamped at build time. |
| `AMPLIHACK_REMOTE_RETRY_BUDGET_SECS` | user preference |  | Retry budget for remote calls. |
| `AMPLIHACK_REPAIR_ORIGINAL_PATH` | internal |  | Caller's PATH handed to the post-update repair. |
| `AMPLIHACK_REPO_PATH` | user preference |  | Default `repo_path` context for `recipe run` and multitask. |
| `AMPLIHACK_REPO_URI` | internal |  | Not an env var: constant in the reflection prompt. |
| `AMPLIHACK_RESULT_SINK` | internal |  | Clean result channel path handed to agent children. |
| `AMPLIHACK_RESULT_V1` | internal |  | Not an env var: result line prefix in the LiteLLM preflight. |
| `AMPLIHACK_ROOT` | internal |  | Legacy root printed by `uvx-help`. |
| `AMPLIHACK_ROTATE` | internal |  | Not an env var: the prefix the profile rotator strips from its probe child's environment. |
| `AMPLIHACK_ROTATE_AUTOSTART` | user preference |  | `0`/`off` stops launches from starting the profile rotator and `1`/`on` lets them, overriding the saved autostart setting; see [Rotate autostart](profile-command.md#rotate-autostart). The test helper `isolated(..)` sets it to `0`. |
| `AMPLIHACK_ROTATE_HOLD` | user preference |  | Default for `amplihack profile rotate --hold`. |
| `AMPLIHACK_ROTATE_INTERVAL` | user preference |  | Default for `amplihack profile rotate --interval`. |
| `AMPLIHACK_ROTATE_MARGIN` | user preference |  | Default for `amplihack profile rotate --margin`. |
| `AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET` | user preference |  | Default for `amplihack profile rotate --overflow-unknown-reset` (`0` or `1`, default `0`); auto-switch reads it only while no rotator runs, and otherwise uses the running rotator's value. |
| `AMPLIHACK_ROTATE_OVERFLOW_WAIT` | user preference |  | Default for `amplihack profile rotate --overflow-wait`; auto-switch reads it only while no rotator runs, and otherwise uses the running rotator's value. |
| `AMPLIHACK_ROTATE_SESSION_AT` | user preference |  | Default for `amplihack profile rotate --session-at`; also auto-switch's plan-capacity test for overflow while no rotator runs. |
| `AMPLIHACK_ROTATE_UPGRADE_MARGIN` | user preference |  | Default for `amplihack profile rotate --upgrade-margin`. |
| `AMPLIHACK_ROTATE_WEEK_AT` | user preference |  | Default for `amplihack profile rotate --week-at`; also auto-switch's plan-capacity test for overflow while no rotator runs. |
| `AMPLIHACK_RUNTIME_DIR` | user preference |  | Worktree runtime directory override. |
| `AMPLIHACK_RUNTIME_ROOT` | internal |  | Runtime root handed to recipe steps. |
| `AMPLIHACK_RUST_BINARY` | user preference |  | Binary for the Rust trial. |
| `AMPLIHACK_RUST_RUNTIME` | internal |  | Marks a process launched by the Rust CLI. |
| `AMPLIHACK_RUST_TRIAL_HOME` | user preference |  | Home for the Rust trial. |
| `AMPLIHACK_SAFE_HEREDOC_V1_TMPWRITE` | test-only |  | Not an env var: heredoc delimiter in a recipe test. |
| `AMPLIHACK_SAFE_HEREDOC_V2_TMPWRITE` | test-only |  | Not an env var: heredoc delimiter in a recipe test. |
| `AMPLIHACK_SAFE_HEREDOC_V3_TMPWRITE` | test-only |  | Not an env var: heredoc delimiter in a recipe test. |
| `AMPLIHACK_SEND_INPUT_ALLOWLIST` | user preference |  | Allowlist for sending input to sessions. |
| `AMPLIHACK_SESSION_DEPTH` | internal |  | Session-tree depth exported to children. |
| `AMPLIHACK_SESSION_ID` | internal |  | Session id exported to children. |
| `AMPLIHACK_SESSION_NONCE` | internal |  | Random value each `amplihack <cli>` launch exports beside its `session:<cli>` tag, so a session can be told apart from a tmux server's copy of another session of the same CLI. |
| `AMPLIHACK_SESSION_TREE_DIR` | internal |  | Session-tree state directory. |
| `AMPLIHACK_SIGNAL` | internal |  | Prefix of the `AMPLIHACK_SIGNAL_*` variables, named in help text. |
| `AMPLIHACK_SIGNAL_ACCOUNT` | user preference |  | Signal account. |
| `AMPLIHACK_SIGNAL_ALLOWLIST` | user preference |  | Signal sender allowlist. |
| `AMPLIHACK_SIGNAL_CONFIG` | user preference |  | Signal config file path. |
| `AMPLIHACK_SIGNAL_ENDPOINT` | user preference |  | Signal endpoint. |
| `AMPLIHACK_SIGNAL_INBOX_CAPACITY` | user preference |  | Signal inbox size. |
| `AMPLIHACK_SIGNAL_OWN_DEVICE_ID` | user preference |  | Signal device id. |
| `AMPLIHACK_SIGNAL_PORT` | user preference |  | Signal endpoint port. |
| `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP` | user preference |  | Opts in to a shared rolling group. |
| `AMPLIHACK_SIGNAL_ROLLING_GROUP_ID` | user preference |  | Rolling group to reuse. |
| `AMPLIHACK_SIGNAL_STATE_DIR` | user preference |  | Signal state directory for hooks. |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | user preference |  | Skips automatic framework install. |
| `AMPLIHACK_SKIP_MMDC` | user preference |  | Skips installing the Mermaid CLI. |
| `AMPLIHACK_SKIP_POWER_STEERING` | user preference |  | Skips the power-steering prompt. |
| `AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL` | user preference |  | Skips installing the recipe runner. |
| `AMPLIHACK_SKIP_REFLECTION` | user preference |  | Skips the stop-hook reflection. |
| `AMPLIHACK_SKIP_UPDATE_CHECK` | test-only |  | Set only by Codex plugin install tests. |
| `AMPLIHACK_SOURCE_ROOT` | test-only |  | Codex front-door contract fixture. |
| `AMPLIHACK_STEP_TIMEOUT` | user preference | `recipe.step_timeout` | Per-step timeout ceiling exported to the recipe runner. |
| `AMPLIHACK_STUB` | test-only |  | Stub binary marker in recipe and remote tests. |
| `AMPLIHACK_SUPPLIED` | internal |  | Not an env var: suffix of the `OVERRIDE_IS_AMPLIHACK_SUPPLIED` constant. |
| `AMPLIHACK_TASK_DESCRIPTION` | user preference |  | Default `task_description` context for `recipe run`. |
| `AMPLIHACK_TEARDOWN_GRACE_SECS` | user preference |  | Grace period between SIGTERM and SIGKILL when tearing down the runner. |
| `AMPLIHACK_TESTTOOL_BINARY_PATH` | test-only |  | `binary_finder` unit-test instance of `AMPLIHACK_<TOOL>_BINARY_PATH`. |
| `AMPLIHACK_TEST_1527_PROBE_ARGS` | test-only |  | #1527 launch-model test probe. |
| `AMPLIHACK_TEST_1527_PROBE_BINARY` | test-only |  | #1527 launch-model test probe. |
| `AMPLIHACK_TEST_BINARY` | test-only |  | Binary under test in contract tests. |
| `AMPLIHACK_TEST_BOOL_UNSET_XYZ` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_DENIAL_PANIC` | test-only |  | Fault injection in the hook protocol test. |
| `AMPLIHACK_TEST_DETACH_START_SECS` | test-only |  | Shortens how long `recipe run --detach` waits for the pane to start the run (default 15 s), so a test of a pane that never starts is quick. |
| `AMPLIHACK_TEST_FAKE_LATEST_VERSION` | test-only |  | Fakes the latest release for update tests. |
| `AMPLIHACK_TEST_HOME_GUARD` | test-only |  | Set only by the test helper `tests/support/isolated_amplihack.rs` on each `amplihack` a test starts; the user-config loader then ignores a HOME outside the temp dir, so no test reads or migrates a real `~/.amplihack`. When it is set and not empty, a launch also skips rotator autostart before any other check, so no test starts a real `profile rotate watch --spawned` daemon; see [Keep Tests from Leaking Rotator Daemons](../howto/keep-tests-from-leaking-rotator-daemons.md). |
| `AMPLIHACK_TEST_MISSING_NO_DEFAULT` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_NONEXISTENT_123` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_NONEXISTENT_456` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_NONEXISTENT_789` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_NONEXISTENT_GET_ERR` | test-only |  | `EnvVar` unit test name. |
| `AMPLIHACK_TEST_PROVIDER_TIMEOUT_MS` | test-only |  | Shortens the workflow provider timeout in tests. |
| `AMPLIHACK_TEST_ROOT_SANDBOX_PROBE` | test-only |  | Root-sandbox spawn test probe. |
| `AMPLIHACK_TEST_SECURITY_MODE` | test-only |  | Hook protocol test mode. |
| `AMPLIHACK_TEST_SECURITY_TRANSPORT` | test-only |  | Hook protocol test transport. |
| `AMPLIHACK_TEST_VAR_12345` | test-only |  | Not an env var: suffix of a uvx-manager test marker. |
| `AMPLIHACK_TIMEOUT` | internal |  | Not read: example name in `amplihack-state` `EnvVar` docs. |
| `AMPLIHACK_TOPIC_NAME` | user preference |  | Hive topic name. |
| `AMPLIHACK_TRACE_FILE` | user preference |  | Trace log file. |
| `AMPLIHACK_TRACE_LOGGING` | user preference |  | Enables trace logging. |
| `AMPLIHACK_TREE_ID` | internal |  | Session-tree id exported to children. |
| `AMPLIHACK_TURN_ERROR_TAIL_BYTES` | user preference |  | Bytes of output kept in a turn error. |
| `AMPLIHACK_UNDER_TEST` | test-only |  | Marks integration-test child processes. |
| `AMPLIHACK_USE_DOCKER` | user preference |  | Runs the agent in amplihack's container. |
| `AMPLIHACK_USE_RECIPES` | user preference |  | Lets the cascade workflow use recipes. |
| `AMPLIHACK_VERSION` | internal |  | Version exported to children. |
| `AMPLIHACK_WORKFLOW_ARTIFACT_DIR` | internal |  | Artifact directory handed to recipe steps. |
| `AMPLIHACK_WORKFLOW_RUNTIME_DIR` | internal |  | Runtime directory handed to recipe steps. |
| `AMPLIHACK_WORKSPACE_MARKER` | internal |  | Not an env var: the install clone's workspace marker constant. |
| `AMPLIHACK_WORKSTREAM_ISSUE` | internal |  | Issue number handed to a multitask workstream. |
| `AMPLIHACK_WORKSTREAM_PROGRESS_FILE` | internal |  | Progress file handed to a multitask workstream. |
| `AMPLIHACK_WORKSTREAM_STATE_FILE` | internal |  | State file handed to a multitask workstream. |
| `AMPLIHACK_WORKTREE_PATH` | internal |  | Worktree path handed to a multitask workstream. |

## Related

- [User Configuration](./user-config.md) — The config files, their keys and the `amplihack config` command
- [Environment Variables](./environment-variables.md) — Detailed behaviour of the variables amplihack sets and reads
