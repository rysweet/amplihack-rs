# Canonical staging regressions

Refs #1554 and Refs #1914.

The [shared test contract](../../../../../../amplifier-bundle/recipes/tests/canonical-staging.md)
defines production scope, complete requirements, evidence identity and distinct
delivery requirements. `coverage.json` contains historical planning statuses;
the shared contract identifies the authoritative current execution ledger.

`inventory.json` records the verified initial settings for 40 authorised agents and the nested terminal adjudicator. The latter remains outside edit scope and still inherits automatic staging. The architecture role includes conditional implementation under its automation-failure contract, so its fixture creates source changes. `step-22-ensure-mergeable` can repair CI after the last source commit checkpoint and has no later recipe source checkpoint. Its CI agent definition illustrates commit/push, but safe directory selection and publication authority remain unverified.

One static policy test checks every authorised agent with per-step diagnostics. Each authorised agent retains its individual actual-runner regression and removal control. The runner consumes a projection of the source recipe, including its staging setting, condition, retry fields and JSON parsing. Only the prompt, execution paths and documented context substitutions change. The private adapter writes runtime metadata and, for mutation roles, source changes. It never stages paths, executes instructions, inspects credentials or calls a model. The parent requirements document is included unchanged as fixture data.

The fixture uses independent caller and review repositories plus a linked task worktree. Every index starts with executable staged contents that differ from working-tree contents and mode. Raw indexes are resolved before execution and copied immediately after return, before Git diagnostics. Staged blobs, modes and NUL-separated entries are checked separately. Generated metadata remains observable; tracked runtime modifications and deferred source changes must remain unstaged. Global runner staging stays enabled.

Every corrected case has a separate fixture that removes only its opt-out. Such a control qualifies only after the runner and adapter complete successfully in the expected directory, the required writes exist, and the intended index-integrity assertion fails. Disabled-staging no-op, inherited Git selectors, linked indexes, deletions, renames, symlinks and unusual names have separate shared cases. A repeated sequence checks parsed-output propagation into a conditional invocation.

The installed runner can be selected with `CANONICAL_STAGING_RUNNER`. Set `CANONICAL_STAGING_EVIDENCE_DIR` to a task-owned report directory and `CANONICAL_STAGING_REQUIREMENTS_FILE` to the complete requirements document. `CANONICAL_STAGING_RECIPE_ROOT` optionally selects mechanically archived original recipe assets for a renewed baseline. Receipts record that source path and its hash. `CANONICAL_STAGING_PRESERVE_DIRECTORY_TEMPLATES=1` retains source directory templates for the additional fidelity checks. Installed binaries outside authorised supporting-read roots are identified through normal version probes; their bytes are not read for a digest.

Run the external tests explicitly:

```sh
cargo test -p amplihack-workflows --locked --test canonical_staging_runner_test -- --ignored
cargo test -p amplihack-workflows --locked --test canonical_staging_checkpoint_test -- --ignored
```

Checkpoint fixtures also require `CANONICAL_STAGING_AMPLIHACK`. They mechanically extract seven unchanged command blocks and use the actual staging, identity and runtime helpers. Normal and rejecting hooks, a local bare remote and a narrowly scoped provider double are fixture substitutions. An attempted repair-agent launch returns a recorded exit 79. That terminal refusal remains failed checkpoint execution, even when a rejecting-hook assertion correctly verifies enforcement. `CANONICAL_STAGING_TOOL_PATH` selects existing system tools without replacing helpers. Bash 5 and GNU tools can exercise the unchanged commands on Darwin; they do not establish supported Linux auto-drive acceptance. Fixture identities use the reserved `example.com` domain accepted by the actual identity helper.

Staging and publication have separate allowlists and exact expected contents. Existing staged user entries grant no publication authority. Checkpoint receipts retain new staged paths, staged modes, every resulting committed path and content, hook snapshots and residual index changes. Unsafe explicit staging remains failed acceptance and requires a separately authorised correction.

`audit_receipts.py REPORT_ROOT --manifest PATH --manifest-sha256 DIGEST` independently checks the complete RED, GREEN and removal-control population against raw snapshots and staged blob evidence. It verifies recipe hashes, source and target directories, requirements hashes and successful adapter execution. A declared amendment can replace affected evidence after an inventory correction while retaining the superseded receipts and reason. Missing cases fail the audit. A raw-byte corruption control confirms that semantic equality cannot excuse a changed index.

The conservative implementation's executed evidence is retained outside source under `/Users/ryan/src/bb/worker-logs/canonical-default-staging-1554/conservative-v1/`. The historical 43 policy tests and 45 tests using resolved directory projections pass, with 40 individual removal controls. Further Crusty controls retain the original directory templates: adapter execution succeeds, but removing the opt-out does not change the index under the installed runner. Those controls fail their intended assertion. The projected results therefore do not establish complete unchanged-template runner behaviour. The corrected checkpoint fixture proves seven normal-hook cases and seven rejecting-hook cases, but its safety and orphaned-publication assertions fail. All seven adversarial checkpoints replace partial staging and publish unauthorised contents. The phase brick-budget check also fails for three files; no comments, commands or helpers were compressed or extracted to conceal it.

Full delivery remains incomplete. These fixtures do not establish all critical consensus branches, final-design approvals, committed-candidate review, native/proof qualification, full quality assurance, required CI, supported-platform auto-drive or owner integration. Darwin lacks the `/proc/self/stat` required by the actual auto-drive host check. The exact qualification commands, 4 GiB mechanism and 595+5-second procedure remain unresolved. Neither broad issue is closed.

Absent, split, sparse and unmerged indexes, concurrent Git writers and interrupted adapters are outside coverage. Complete retry, resume and terminal-repair sequences remain unexecuted. The final-candidate evidence test remains separately invoked and requires genuine, complete parent receipts.
