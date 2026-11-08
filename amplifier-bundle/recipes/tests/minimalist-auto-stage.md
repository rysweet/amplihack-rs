# Canonical automatic staging checks

The [shared test contract](canonical-staging.md) defines production scope,
requirements, evidence identity and separate delivery requirements for
Refs #1554 and Refs #1914.

The minimalist auto-stage tests in
`crates/amplihack-cli/tests/workflow_checkpoint_auto_stage.rs` require an
installed `recipe-runner-rs`; without it they stay `#[ignore]` with a reason
that names the runner. They call the actual runner with its automatic staging default enabled and
inject a task-private launcher through `AMPLIHACK_LAUNCHER_BINARY`. The
launcher writes metadata and optional source changes, records its directory,
and returns JSON. It invokes no model and reads no credentials.

Pass the runner, report directory and requirements file through
`CANONICAL_STAGING_RUNNER`, `CANONICAL_STAGING_EVIDENCE_DIR` and
`CANONICAL_STAGING_REQUIREMENTS_FILE`. The
requirements file must contain the complete supplied requirements unchanged.
The historical baseline is pinned to the assigned initial commit, so committing
the correction does not change RED inputs. Select agent-integrity or explicit-checkpoint cases with a test-name filter;
neither selection establishes full acceptance. Keep the report and `TMPDIR`
outside the source checkout. Run the command
detached when it may take more than 30 seconds.

The tests verify recipe membership through `default-workflow.yaml`, check
that the only parsed recipe changes are the authorised opt-outs, and record
the nested terminal adjudicator without changing it. They identify the
mergeability repair role's unverified publication authority. Their baseline, corrected
and independent removal cases use separate temporary repositories. Each case
observes the caller, review, primary and linked-worktree indexes. It compares
raw bytes before any subsequent Git diagnostic and separately compares staged
blob identifiers and modes. The fixtures include partial staging and an
executable mode difference. Additional cases cover staged deletions, renames,
symlinks, spaces, tabs, newlines, Unicode, leading hyphens and pathspec syntax.
The disabled-staging no-op case checks runner bookkeeping separately.

The default directory projection preserves the original templates. The
optional resolved-directory projection substitutes the directories
selected by the supplied context. It records this substitution explicitly.
Passing the latter projection does not establish native template staging
behaviour: the locally available runner renders the agent directory but
attempts automatic staging at the original template. A removal case that
leaves the index unchanged is invalid evidence of the opt-out's necessity.

Checkpoint checks extract the two canonical checkpoint commands and Crusty's
commit block mechanically. They execute the blocks with actual source
helpers, a local bare remote and successful or rejecting hooks. Staging and
publication have separate allowlists, both limited to the intended source
path and contents. Existing staged entries confer no publication authority.
On Darwin the fixture selects the existing GNU `mktemp` through its private
`PATH`, because BSD `mktemp -t` selects a directory outside the task sandbox.
The rejecting hook's final `fatal:` line selects the helper's existing
non-repair failure route. Repair-route acceptance remains untested. Any
unexpected repair invocation is refused before a model can run. These
differences are retained in each receipt.

The tests fail if case accounting is incomplete, a
control is invalid, index preservation fails, or checkpoint acceptance fails.
Expected control failures are recorded separately from failed acceptance.
They never weaken checkpoint assertions to make the bounded correction pass.

Absent, alternate, split, sparse and unmerged indexes, concurrent Git
activity, interrupted adapters and non-UTF-8 filenames are outside these
focused fixtures. Full workflow sequences, consensus branch execution,
independent review, resource qualification, full quality assurance, required
CI, supported-platform auto-drive and owner integration require separate
evidence against the final candidate. These tests establish none of those
outcomes. The issues remain open while their broader requirements remain
unresolved.
