# Troubleshoot Recipe Execution

Common recipe execution problems and their solutions.

## Shell step hangs in non-interactive mode

**Symptom**: A shell recipe step hangs waiting for user input.

**Cause**: The shell command invokes a tool (e.g., `apt-get`) that prompts
for confirmation, and the recipe executor's environment didn't signal
non-interactive mode.

**Fix**: As of the #277 fix, the recipe executor injects these environment
variables into every shell step:

- `NONINTERACTIVE=1`
- `DEBIAN_FRONTEND=noninteractive`
- `CI=true`
- `HOME` (preserved from caller, fallback to `/root`)
- `PATH` (preserved from caller, fallback to standard paths)

If a tool still prompts despite these, add `-y` or `--yes` flags to the
command explicitly.

## Agent step produces zero file changes

**Symptom**: An agent recipe step completes successfully but creates no files.

**Cause**: The agent doesn't know the working directory or what files exist
to modify.

**Fix**: As of the #251 fix, agent steps receive a `Working Directory Context`
block in their prompt containing the absolute path and file listing. If the
issue persists, verify the recipe YAML includes a `working_directory` context
field.

See also: [Recipe Executor Environment](../reference/recipe-executor-environment.md)

## python3 not found during shell step

**Symptom**: Shell step fails with "command not found" or "permission denied"
when invoking `python3`.

**Cause**: The recipe host doesn't have Python installed, but the shell
command references it.

**Fix**: As of the #242 fix, the executor validates shell prerequisites
before execution and reports missing tools clearly. Install the required
tool or rewrite the step to use Rust-native alternatives.

## Task misclassified as Ops instead of Default

**Symptom**: A constructive task like "Add a feature to manage users" is
classified as OPS_WORKFLOW and completes instantly without doing work.

**Cause**: The word "manage" (or similar ops-like keywords) triggered OPS
classification despite the constructive intent.

**Fix**: As of the #269 fix, OPS keywords are multi-word phrases and a
constructive-verb override detects verbs like "add", "create", "build" to
force DEFAULT classification.

See also: [Workflow Classifier Reference](../reference/workflow-classifier.md)

## Stale framework assets after update

**Symptom**: After running `amplihack update`, skills or recipes still show
old behavior.

**Cause**: Prior to the #249 fix, `amplihack update` only replaced the binary
without re-staging framework assets.

**Fix**: `amplihack update` now calls `ensure_framework_installed()` after
binary replacement. If it fails, run `amplihack install` manually.

## Install downloads from wrong repository

**Symptom**: `amplihack install` downloads from the Python repo instead of
the Rust repo, resulting in missing `amplifier-bundle/` directory.

**Cause**: Prior to the #254 fix, the URL constants pointed to
`rysweet/amplihack` (Python) instead of `rysweet/amplihack-rs` (Rust).

**Fix**: URLs now point to `rysweet/amplihack-rs`. The
`find_framework_repo_root()` function accepts both `.claude/` and
`amplifier-bundle/` directory markers.

See also: [Install Command Reference](../reference/install-command.md)

## Checksum download fails during update

**Symptom**: `amplihack update` fails with a network error when downloading
the SHA-256 checksum file.

**Cause**: Transient HTTP errors (502, 503) from GitHub's CDN.

**Fix**: As of the #257 fix, checksum downloads use `http_get_with_retry()`
with exponential backoff (3 attempts, 500ms initial delay).
