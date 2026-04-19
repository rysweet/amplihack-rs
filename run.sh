#!/bin/bash
cd /tmp/amplihack-workstreams/ws-262
# Propagate session tree context so child recipes obey depth limits
export AMPLIHACK_TREE_ID=e0381056
export AMPLIHACK_SESSION_DEPTH=2
export AMPLIHACK_MAX_DEPTH=6
export AMPLIHACK_MAX_SESSIONS=10
# Bake in the detected delegate so nested ClaudeProcess inherits it (S2)
export AMPLIHACK_DELEGATE='amplihack claude'
export AMPLIHACK_WORKSTREAM_ISSUE=262
export AMPLIHACK_WORKSTREAM_PROGRESS_FILE=/tmp/amplihack-workstreams/state/ws-262.progress.json
export AMPLIHACK_WORKSTREAM_STATE_FILE=/tmp/amplihack-workstreams/state/ws-262.json
export AMPLIHACK_WORKTREE_PATH=''
# Invoke recipe runner directly via Rust CLI
exec amplihack recipe run default-workflow \
-c 'task_description=Fix 4 doc-vs-code drift bugs and add reference docs for 4 undocumented commands (issue #259 part B). Drifts: (1) docs/reference/launch-flag-injection.md says --dangerously-skip-permissions always injected but code only injects for Claude-compatible tools per crates/amplihack-cli/src/commands/launch/command.rs:39-49 — fix doc. (2) Same doc says --resume/--continue work on launch AND claude but Commands::Claude has no such fields per crates/amplihack-cli/src/cli_commands.rs:23-96 — fix doc. (3) docs/reference/completions-command.md:83-92 shows nonexistent subcommands — replace with real commands from cli_commands.rs:13-337. (4) docs/reference/memory-index-command.md:147-166 says index-scip outputs JSON but code prints plain-text per crates/amplihack-cli/src/commands/memory/scip_indexing/commands.rs:43-70 — fix doc. Add new reference docs: rustyclawd-command.md, uvx-help-command.md, resolve-bundle-asset-command.md, multitask-command.md. Open a PR and set auto-merge.' \
-c repo_path=. \
-c issue_number=262 \
-c workstream_state_file=/tmp/amplihack-workstreams/state/ws-262.json \
-c workstream_progress_file=/tmp/amplihack-workstreams/state/ws-262.progress.json \
-v
