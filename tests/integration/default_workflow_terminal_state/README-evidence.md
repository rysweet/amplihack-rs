# Local workflow evidence contracts

`workflow_terminal_evidence.sh` observes commands and validates sealed outputs.
Requests use integer `schema_version: 1`, canonical `repository_root`, and the
immutable `workflow: {name, started_at, original_run_id?}`. The framework's
installation directory does not select the task repository. Selected-file
transport takes precedence over environment aliases. Only declared empty recipe
defaults count as missing; malformed, null and negative records retain authority.

The stock runner's `repo_path` may name the primary checkout when an explicit
`worktree_setup.worktree_path` selects the invoking task worktree. Git must prove
that checkout owns the task's common Git directory. A sibling worktree, clone,
missing selection or mismatched canonical `repository_root` is rejected. Source
and capture bindings always retain the task worktree and its actual HEAD.

`checkpoint-implementation` is passive. It stores all original outputs, including
unsuccessful attempts, and atomically publishes the latest generation. `verify`
does the same for verification. Restoration reads that latest generation only.
Neither operation searches history for success. An unsuccessful generation sets
an observation barrier: replaying an earlier positive report cannot clear it.
New producer/verifier observations or new check executions are required. Reports
must also include each obligation's latest execution attempt. Successful tests cannot supply
implementation provenance, and new implementation cannot refresh old tests.

The stock verifier emits optional investigation prose followed by a final JSON
line with `verdict`, `evidence` and `rationale`. No `head` or `artifacts` extension
is required. Original bytes are retained. The adapter gives the stock terminal
producer a parsed view of the final line without rewriting original output.

The existing recipe runner's `StepResult` and `RecipeCheckpoint` have no source
observations around producer/verifier execution. A controller must run genuine
producer and verifier subprocesses through `observe-implementation` with
`producer_argv`, `verifier_argv` and canonical `cwd`. This observes child process
exits, source before/after, exact output files, tools, script inputs, configuration
and environment. Both processes execute; importing output or relabeling a step
cannot create this origin record. The helper is an execution recorder, not an
independent judge of the verifier's reasoning.

**Controller dependency:** place that observer around `workflow-tdd.yaml`'s
`step-08-implement` and `step-08c-work-verifier` (including a genuine refresh on
resume when necessary). Those producing points and runner source are outside
this assignment's seven-entry scope. No automatic refresh is inferred from a
passive checkpoint. Until the controller supplies real observed execution,
implementation remains `UNPROVEN`.

Plans bind all applicable components, inspected policy, configured commands and
canonical execution directories. Rust requires exactly
`cargo test --workspace --locked`, `FULL_WORKSPACE`, and `DEFAULT` test threading.
Compilation concurrency is separate. Other toolchains retain their prescribed
commands. Hooks follow Git resolution and the user XPIA exclusion; skipping other
hooks cannot establish success.

Each consumer scenario supplies `role`, `assertions`, `inspected`, `entry_points`,
`cases` and `assertion_refs`. Entry points and assertion references identify
repository-relative executable source. Captured argv selects those files and
cases. Native harness results must show passing selected tests. Direct scenario
programs emit the case name and `consumer assertions passed` after assertions.
Basic and edge/integration use distinct executions. QA assesses whether the
assertions establish the required product behavior.

All later validation rechecks source, executable/script inputs, effective
configuration and environment. Internal source symlinks bind link text and target
content; external, cyclic or unresolved source links remain unproven. Artifact
hashing and log scanning use bounded descriptor reads, including matches across
chunk boundaries. Structured documents and numeric receipts have size limits.

Final-status, collection, validation and completion use the same local evidence
postconditions. Collection and classification verify canonical repository paths
and full HEAD before using completion flags. Explicit failed/cancelled/error CI
prevents proposed success regardless of positive terminal metadata. Existing
harder unsuccessful classifications and publication/merge gates remain binding.
The controller still owns native .NET, full workspace proof, current-head review,
QA, CI and merge through its evidence gate. Historical 2204 evidence retains its
original identity and `FAILED_IMPLEMENTATION`; server withholding/recheck remains
`UNMET` without actual server proof.
