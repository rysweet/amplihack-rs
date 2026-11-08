# Canonical staging checks

The DEFAULT workflow and `autodrive-crusty-round.yaml` set `auto_stage: false`
on the original 40 agent steps. The nested terminal adjudicator remains outside
that population. Three new read-only path-scope reviewers and the nested repair
agent have four separately counted opt-out controls. The generic runner default
and the consensus recipe assets are unchanged.

Publish, review-feedback and final-cleanup checkpoints capture pending source
and complete outgoing history before requesting a finite path decision.
Their extracted helpers validate it before staging, commits, repair retries
and publication. Files are identified by literal filename bytes.
Pre-existing staging without task permission must refuse before mutation.
Failures after authorised operations preserve the resulting work and error.

The only Cargo change is the existing workspace `amplihack-git` development
dependency for workflow tests and its lockfile edge. Source checks verify the
exact approved Cargo inputs. Refs [#1554](https://github.com/rysweet/amplihack-rs/issues/1554)
and [#1914](https://github.com/rysweet/amplihack-rs/issues/1914) remain broader
than these changes.

## Source and fixture inputs

Use the repository's pinned Rust toolchain. Supply the complete task contract
through `CANONICAL_STAGING_REQUIREMENTS_FILE`. Keep reports, temporary
repositories and build outputs outside the source checkout. `TMPDIR` must name
an assigned executable directory that is not inside a Git worktree.

The common diagnostic fixture reads the exact retained pre-delta assets from
[checkpoint-pre-delta-assets.json](../../../crates/amplihack-workflows/tests/fixtures/canonical_staging/checkpoint-pre-delta-assets.json), checking
the complete fixture hash and each asset hash. Current staging and later commits
cannot replace that historical reference.

The separate scope audit requires commit
`606f3c746e51b8ef66d75380b41791b9f3a9ed8e` and the task's ancestry to be hydrated
before offline execution. It inspects working, staged and committed changes.
Missing ancestry is a preparation failure, not an empty successful audit.
Do not fetch from inside a private test to conceal missing preparation.

Use isolated synthetic home directories and the approved runtime for compiled
CLI tests. Do not provide real model profiles, credentials or personal home
directories. Record actual tool versions, source hashes and binary hashes.
Keep default test parallelism and the execution limits assigned to the job.

## Diagnostic suites

The checkpoint suites are Rust integration tests. They run the cargo-built
`amplihack` binary as a subprocess against newly created local bare
repositories, with a cleared environment, a private `HOME` and no global or
system Git configuration. Their scope review is an independent synthetic owner
decision. No provider or review model runs. They run in plain
`cargo test --workspace --locked` and in CI nextest.

| Rust test file | Former Python suite | Responsibility |
| --- | --- | --- |
| `crates/amplihack-cli/tests/workflow_checkpoint_ports.rs` | `test-checkpoint-scope.py` | Literal staging, invocation authority, refusal, repair and publication history. |
| same | `test-checkpoint-conservative-boundaries.py` | Strict paths and authority identity. |
| same | `test-checkpoint-synthesis-alias.py` | Branch aliases and exact filename spelling. |
| same, and `tests/integration/checkpoint_owner_controls.rs` | `test-checkpoint-publication-boundaries.py` | Configuration substitution, publication range and shallow history; the 20 `OwnerOutgoingOnlyControls` cases. |
| same | `test-checkpoint-review-boundaries.py` | Refusal logging, terminal fetch status, inventory and fixture preparation. |
| same | `test-checkpoint-scope-history.py` | Staged and committed scope, exact Cargo inputs and independent second-parent coverage. |
| same | `test-checkpoint-rebase-identity.py` | Significant whitespace, lossless replay and private object writes. |
| same | `test-checkpoint-refusal-evidence.py` | Admission evidence and refusal state. |
| same | `test-checkpoint-value-transport.py` | Fixed data-only fields, exact framing and hostile-data refusal. |
| `tests/integration/checkpoint_exit79.rs` | none (new) | A terminal 79 from the destination read stops step 15 with 79 and no push. |

Each Rust test is named after its Python method without the `test_` prefix and
keeps the same cases and assertions. The PR #2020 mapping comment lists every
method and its Rust path. Invalid-UTF-8 filename cases that fail at creation
with `EILSEQ` are recorded as unavailable, rather than passed. Other creation
errors remain failures. Linux executes the unchanged cases. A partial platform
result does not establish complete acceptance.

The binary these tests call is described in the
[`amplihack internal workflow-checkpoint` reference](../../../docs/reference/workflow-checkpoint-cli.md).

## Native matrices and checkpoints

The Rust policy and evidence targets reconcile the original 40 steps and four
additions. `crates/amplihack-cli/tests/workflow_checkpoint_auto_stage.rs` retains the
minimalist and pragmatic native, resolved and individual removal cases. Cases
that need an installed `recipe-runner-rs` stay `#[ignore]` with a reason that
names it; pass the runner, baseline, candidate, report and requirements file
through the documented `CANONICAL_STAGING_*` variables. Resolved directory projections
do not qualify native implicit staging.

The Rust checkpoint tests retain all seven checkpoint sites. Positive fixtures start with independently clean linked task
indexes. Unrelated staging in caller and review worktrees remains observable.
Negative fixtures preserve partial staging, unknown metadata, raw indexes,
blobs, modes, worktrees and heads. Normal and rejecting hooks must execute
in their intended positive cases.

Only publish, review-feedback and final-cleanup have the new bounded production
scope mechanism. Their test projections execute deterministic capture and use
finite synthetic owner permissions before invoking the real checkpoint.
The internal `--bounded-fixture-projection` mode is for disposable test
repositories only. It does not replace the production model decision.

The other four sites retain their existing production behaviour. Record their
actual results rather than dropping them from either inventory. Existing
ignored Rust acceptance targets require explicit execution; a normal workspace
test result does not establish those ignored requirements.

## Delivery evidence

Runner [#201](https://github.com/rysweet/amplihack-recipe-runner/issues/201)
concerns implicit staging with an unresolved working-directory template.
The original 25 affected native removal controls require the owning runner
correction, a release and an authorised pin update. Materialising paths cannot
satisfy that requirement.

The mergeability role still requires an independently established publication
owner. No source audit, mocked scope decision or recipe success status proves
delivery. Retain exact-source critical review, normal hooks, complete QA,
required CI, supported Linux auto-drive evidence and owner integration
separately. Keep the broader issue references open until their requirements
are established.


Inspection of filtered files uses Git's NUL-delimited configuration-name interface. It does not read operator command values. The working-file and cached-index queries temporarily use a qualified, root-owned `/bin/cat` identity filter and disable process filters only for that query. They do not change repository or global configuration. Normal authorised staging retains its configured clean/process filters and their required status.

This inspection is not equivalent to every arbitrary normaliser. A stat-dirty file can be listed as a candidate even when an operator filter would make it logically clean. The capture states this limitation. A candidate receives no additional task permission: an unknown path still requires refusal before staging. The transforming clean/process controls verify no operator invocation during capture, exact uppercase index blobs after explicit finite authority, unchanged working bytes and unchanged real filter configuration. Unused handler and unowned-path controls remain separate.

Publication captures outgoing history against the privately resolved push destination. An existing task reference excludes its advertised ancestry. A new task reference excludes ancestry from every advertised remote reference, including an advertised `HEAD` and commit ancestry reached through tags. Fetch tracking remains separate for the original rebase checks. Both already-pushed decisions inspect the same actual push destination. Inherited or repository grafts are refused before mutation. The source-scope audit requires complete ancestry, a real ancestor base and history unaffected by replacement refs.

A capture can inspect a newly advanced destination without downloading its
objects into the implementation repository. It prepares one task-owned private
Git evidence repository, imports already-owned history through a streamed pack
and fetches only the captured destination tip into that private repository.
Preparation records its private writes. The implementation object inventory,
refs, tracking, `FETCH_HEAD`, index, staged blobs, worktree and `HEAD` remain
unchanged before permission. Missing owned objects must cause refusal rather
than an implicit source-side fetch.

The retained history descriptor binds its canonical directory, filesystem
identity, object format, complete retained inventory and destination identities.
Every use validates that evidence. Admission and repair reuse it without a new
fetch or renewed scope. The original later fetch, rebase and push operations
remain distinct. Native Git range comparisons cover divergent, merged and
unrelated histories as well as ordinary ancestry, including every parent and
its changed paths. Failed preparation refuses; exit 79 remains terminal.

Scope decisions remain raw until the local parser checks unique fields, valid
Unicode, finite JSON numbers and the complete typed binding. Error messages do
not repeat untrusted field names. Review-feedback and cleanup commit output
uses each phase's existing URI redactor on both successful and rejected hooks,
while retaining the original output channel and exit status. Focused fixture
results establish those behaviours only; they do not replace whole-source
checks, native controls, final review or publication evidence.


## Dated publication requirement: decision 17.116

The owner decision of 6 November 2026 replaces admission based on every new
local commit object with checks on the actual outgoing publication range.
An unreachable local commit is no longer a reason to refuse publication.
The former unreachable-commit refusal and its earlier failed controls remain
historical evidence. Object inventories can still be recorded as diagnostics;
they do not grant or deny task permission by themselves. The tests explicitly
record this changed requirement rather than claiming the old assertion passed.

Before each actual push, the helper uses the bound push destination's own
advertisement. The same process-local view supplies its range checks. Local
tracking references and `FETCH_HEAD` cannot shrink that range. Every outgoing
commit is checked against every parent, with its literal changed paths.
Advertised history grants no path permission to a local commit or merge.
Unknown pending or staged source still refuses before mutation.

The helper permits only the one task reference selected by the original
checkpoint commands. Extra references, mirror or tag expansion, configured
push refspecs, alternate push routing and recursive submodule publication are
refused. It retains the original normal hooks and supported explicit and
implicit task-branch forms. Fetch and push URLs can differ; the push URL and
advertisement determine publication ancestry.

A new reference requires enough already-owned or privately prepared history
to interpret the advertised ancestry. Known tree/blob references and tags
without commit ancestry add no excluded commits. Unavailable advertised
objects cause conservative refusal. The helper performs no extra fetch and
uses no prepared-base, tracking or `FETCH_HEAD` fallback for that case. This
limitation is explicit in the distinct-route controls.

A failed or partial fetch or pull ends the attempt nonzero before push, with
its actual status, redacted diagnostics and authorised local effects retained.
The old publish-phase cached-upstream success fallback is withdrawn. This
implementation chooses the owner's permitted first-failure stop for pull;
the owner also permits truthful retries, but does not require them. Normal
push retries remain unchanged. A successful fetch followed by an unreadable
push advertisement also stops before push. Exit 79 remains terminal.

There is no import receipt, extra fetch advertisement or fifth operation
field. Admission retains eight fixed fields and later operations retain four.
The fixtures cover all nine sibling cases, late siblings, forged tracking and
`FETCH_HEAD`, unauthorised commit/merge/rebase histories, new references,
remote merges, distinct routes, configured expansion, unavailable ancestry,
failed/partial fetches and unreadable advertisements. Native controls and
final delivery requirements above remain separate.


## Independent V21 fetch correction

The independent conditions review of manifest `0a99dfb0` refused acceptance because cleanup's separate sync helper converted its nested fetch failure, including terminal 79, into success. The dated BB instruction and root confirmation authorise the corresponding `workflow_pr_head_sync.sh` change. Cleanup now selects `--strict-fetch`, captures the private sync output, redacts it before emission and stops on its actual nonzero status before any later pull or push. The helper's no-argument behaviour and branch/fast-forward logic remain unchanged. Only this strict cleanup callback receives nested-fetch failure precedence if post-inspection also refuses; terminal 79 still has priority.

Focused controls target ordinary failure, partial writes and terminal refusal inside that nested transport, including a failed fetch followed by unreadable advertisement. Separate controls execute forged tracking/FETCH_HEAD refusal through the shell push path with status 78 and inject malformed advertisement bytes with transport status zero. Earlier passing controls and the independent refusal remain retained; they are not relabelled as acceptance of the correction.


## Successor conditions and early sync

The dated 17.116 amendment permits one advertisement read in the earlier finalization sync's opt-in mode. The command uses the same full validated advertisement shape as the push inspection, against the effective **fetch** origin. It decides only whether the exact task ref exists. It never supplies the outgoing publication range, is never cached across invocations and creates no receipt or additional result field. Missing or malformed PR metadata is not absence evidence. A failed, unreadable or malformed advertisement stops before fetch or push; a proved absent ref skips the fetch, while a present ref selects strict fetching. The actual earlier recipe command captures and redacts diagnostics and preserves its nonzero status, including terminal 79.

Cleanup adds no advertisement read. Its existing `before(sync)` check already has the fresh push advertisement. A first-publication skip requires initial and current exact-ref absence, no task upstream, a single configuration-resolved fetch URL equal to the bound push URL, and the exact cleanup callback/ref. The decision is retained only in the existing hash/identity-bound private operation token. Token type, operation, callback, reference, invocation, file identity and authority are checked before the skip is honoured. Distinct fetch/push destinations cannot borrow push-side absence. Every actual failed or partial fetch remains fatal, and the generic no-argument sync helper retains its original best-effort behaviour. The eight admission and four operation-result fields are unchanged.

The forged-range shell control now starts with genuinely published and fetched unrelated history. After the authorised wrapped source commit, only its disposable bare task ref is rewound to the known earlier base. Held HEAD/upstream remain genuine. The real outgoing range therefore exposes the previously remote unrelated history; the required refusal must name outgoing scope. Two disposable helper mutants that exclude ancestry using tracking or FETCH_HEAD actually publish that history and are rejected by the same oracle. These local fixture publications are not GitHub publication or production acceptance. The original twelve owner test methods retain their assertions; the later insensitive control and all failed preparations remain historical evidence.

The exact earlier-call/status/redaction sequence and amended comment have finite preservation mappings in the Rust inventory tests. Raw and parsed comparisons still cover every other command, field and original opt-out. Mutation controls reject missing flags, status laundering, wrong channels, duplicate calls and extra commands. Inventory preparation deliberately stops before native execution. Normal committed-head checks, independent delta approval, CI and protected canonical integration remain required; neither a fixture result nor recipe completion substitutes for them.


## Rust checkpoint helper: decision 17.126

The repository ships no Python. Decision 17.126 grants no exception for the
checkpoint, so the publish, review-feedback and cleanup helpers are part of
the `amplihack` binary, behind the hidden subcommand
`amplihack internal workflow-checkpoint <action>`. The tools and recipes call
it as `"${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint …`. No
recipe or tool step needs `python3` for a checkpoint, and
`scripts/check-no-python-assets.sh` passes unchanged. The actions, exit
statuses, environment and JSON byte format are in the
[`amplihack internal workflow-checkpoint` reference](../../../docs/reference/workflow-checkpoint-cli.md).

Every 17.116 rule above is unchanged. Two step-15 call sites change one
detail: both `destination-is-head` reads now exit with the real status, so a
terminal 79 from the destination read is reported as 79, not 1.
`tests/integration/checkpoint_exit79.rs` puts a Git wrapper first on `PATH`
that exits 79 for the destination read, and checks exit 79, the stderr
diagnostic and an unchanged bare remote at both sites.

The `verify-pushed-ref` call still maps its verifier failure to 1. That is a
separate follow-up and not part of this change.

The `python3` uses already present in `git-identity.sh`,
`workflow_sync_package_version.sh` and `autodrive_qa_sources.sh` predate this
change and are outside it; the last one is a pattern that matches the word in
a command string, not a call.

Some evidence tests still compare exact script text, because the behaviour
they cover cannot be observed without a native runner or model CLI. The PR
#2020 body lists each one with its reason. Every other former string check is
a behavioural test against the binary.
