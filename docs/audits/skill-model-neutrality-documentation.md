# Model-neutrality documentation review evidence

## Evidence status and acceptance specification

The reference and usage guide describe the finished-state acceptance contract.
Recursive fail-explicit loading and metadata support were pending at the original review and are now implemented in this branch. Supporting-YAML and isolated full-corpus publication now have dedicated regressions; broader asset checks remain implementation specifications; the evidence below identifies only checks actually run. Historical results below retain their original
scope and do not validate a subsequently reconciled revision.

The parent `skill-load-tests.log` records 11 passing name tests and four passing
frontmatter type tests. The parent `real-skill-loader-tests.log` records a failing
real-bundle production-loader regression: nine identities were omitted, including
all seven nested skills, `amplihack-expert`, and `dynamic-debugger`. The latter two
skills carry structured `token_budget` mappings. These observations establish
why YAML syntax success is insufficient; they do not establish a corrected
loader result.

Final implementation evidence records exact 130-name and 130-path equality,
nonempty bodies, directory resolution, metadata and structure results,
supporting-YAML counts and exclusions, isolated publication results, neutrality
and mirror checks, review outcomes, and tested revision. SDK compilation and
runtime checks remain separate. Commit, push, PR URL, and PR check results are
recorded only after those actions occur.


This report records the documentation refinement step. Current behavior and the intended configuration contract are described in [the reference](../reference/skill-model-neutrality.md) and [the usage guide](../howto/skill-model-neutrality.md); configuration validation is checked statically in the target worktree.

## Existing work preserved

The 71-file statement described an earlier staged snapshot; it does not describe the later 76-path reconciliation manifest. Those are different snapshots, not interchangeable totals. The copied root snapshot contains 76 tracked changed paths (listed in `/d0/ryan/sunfixamp-model-neutrality/refinement-source-manifest.txt`); adding the preserved untracked regression and loader fix gives 78 paths in this branch. The current refinement copied the root tracked diff into the clean `feat/issue-1533-user-explicitly` worktree and preserved the untracked real-bundle regression. Root edits remain untouched. The three refined documentation pages were copied from `feat/issue-1532-complete-model`.

## Scope and decisions

Filesystem enumeration confirms 130 canonical skills (123 top-level, seven nested), 537 accessible canonical text paths, and 323 mirror text paths containing 81 mirrored skills. All paths were read to establish text coverage; this step does not claim a fresh semantic audit of all their contents. The inventory below identifies each canonical skill and mirror availability.

The documentation specifies removal of execution selections, runtime inheritance where supported, explicit required provider configuration, distinct primary/fallback inputs, Azure deployment semantics, and the intended pre-client validation of absent, empty, whitespace-only, and unresolved placeholder values in all three C# examples. The examples and their mirrors reject absent, empty, whitespace-only, and angle-bracket placeholder values with `string.IsNullOrWhiteSpace` and delimiter checks before constructing clients. API parameter names and credentials remain provider-specific. Historical research, poetic forms, and reviewed API context remain allowed; exact exceptions require reasons and cannot cover whole files.

Directory links from `docx` and `pptx` point to canonical `common/ooxml`; `outside-in-testing/examples` points to `qa-team/examples`. Directory links are not recursively followed. The accessible `outside-in-testing/README.md` file link is read and counted. Its `scripts` and `tests` links point to missing `qa-team` directories; no contents behind those links were available for review.

## Validation evidence and limits

Earlier statements about whitespace, links, fences, mirror checks, and caller-directory independence lacked revision, exit-code, and log attachments. They remain historical reports, not reproducible evidence for this branch. Parent frontmatter logs likewise omit a recorded tested revision; they are baseline evidence only. Baseline loader failure is retained at `/d0/ryan/sunfixamp-model-neutrality/real-skill-loader-tests.log` (nine omitted identities).

Current production-loader validation checks exactly 130 recursive files, exact name equality, resolved source-directory equality, and nonempty bodies. Unit tests cover optional description, extension round trips, typed metadata rejection, duplicate names, malformed delimiters, and empty bodies. Extension preservation does not add execution semantics.

At this historical revision, supporting-YAML, corpus structure/assets, and complete isolated installation coverage were still planned. The follow-up evidence below supersedes the YAML and installation limitations. SDK compilation and provider execution remain unverified; C# configuration rejection is already implemented in all three examples and mirrors, so it is not pending implementation.

Builds use the authorized existing cache `/d0/ryan/amplihack-builds/target/amp`; temporary files use `/d0/ryan/sunfixamp-model-neutrality/tmp`. Evidence for this refinement is recorded below with exact commands and log paths. The PR remains open and unmerged.

## Recorded refinement validation

Tested tree: base revision `4ccd1977a54e06a7f940507ebc2041f2f638869e` plus this branch’s uncommitted 78-path change set, subsequently committed for publication. Commands ran from `feat/issue-1533-user-explicitly`, with `CARGO_TARGET_DIR=/d0/ryan/amplihack-builds/target/amp` and `TMPDIR=/d0/ryan/sunfixamp-model-neutrality/tmp`.

| Command | Exit | Log |
| --- | --- | --- |
| `cargo test -p amplihack-domain-agents --test bundled_skill_catalog` | 0 | `/d0/ryan/sunfixamp-model-neutrality/loader-final.log` |
| `cargo test -p amplihack-domain-agents --lib skill_catalog` | 0 | `/d0/ryan/sunfixamp-model-neutrality/metadata-final.log` |
| `cargo test -p amplihack --test skill_frontmatter_name --test skill_frontmatter_type --test issue_849_skill_mirror_citation` | 0 | `/d0/ryan/sunfixamp-model-neutrality/frontmatter-final.log` |
| `cargo test -p amplihack-cli issue_1438_skill_publication` | 0 | `/d0/ryan/sunfixamp-model-neutrality/publication-final.log` |
| `bash amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh` | 0 | `/d0/ryan/sunfixamp-model-neutrality/neutrality-final.log` |
| `PYTHONDONTWRITEBYTECODE=1 python3 amplifier-bundle/recipes/tests/test_skill_model_neutrality.py` | 0 | `/d0/ryan/sunfixamp-model-neutrality/regressions-final.log` |
| `bash amplifier-bundle/recipes/tests/test-issue-962-skill-mirror-parity.sh` | 0 | `/d0/ryan/sunfixamp-model-neutrality/mirror-final.log` |
| `bash -n amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh` | 0 | `/d0/ryan/sunfixamp-model-neutrality/syntax-final.log` |
| `git diff HEAD --check` | 0 | `/d0/ryan/sunfixamp-model-neutrality/whitespace-final.log` |
| `shellcheck amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh` | 0 | `/d0/ryan/sunfixamp-model-neutrality/shellcheck-final.log` |

The loader regression passed for all 130 names and resolved paths; 11 catalog unit tests passed, 11 frontmatter name tests, four type tests, six citation tests, one existing installation test, and six neutrality regression tests passed. The neutrality guard reports 130 skills and 860 text paths. Documentation local-link and paired-fence checks exited 0 (`/d0/ryan/sunfixamp-model-neutrality/documentation-final.log`); the script reads only the three refined pages. Root preservation was checked by `cmp` of `tmp/root.patch` and `tmp/root-after.patch` (exit 0).

Review and double-check examined loader discovery, error propagation, metadata serialization, typed operational fields, exact corpus coverage, configuration validation and documentation claims. The citation target names were confirmed against Cargo registrations; no command rename was warranted. Remaining planned tests are disclosed above. Changing `SkillMeta.token_budget` from `Option<u32>` to `Option<TokenBudget>` is a Rust API change: callers matching scalar budgets must use `TokenBudget::Scalar`. Repository callers compile in the completed tests.

Pre-commit initially exited 1 for a Rust formatting mismatch (`precommit-final.log`); `cargo fmt -p amplihack-domain-agents` corrected it. `cargo fmt --all --check` then exited 0 (`format-final.log`), and Clippy passed. The staged-tree pre-commit recheck exited 0 and is logged at `/d0/ryan/sunfixamp-model-neutrality/precommit-recheck.log`.

CI on commit `740b133b83ce3aa18e844fc227acb54f478d86a4` rejected the copied `.py` asset under the repository’s tracked-Python guard. The six regression cases were moved into `test-skill-model-neutrality-regressions.sh`, initially using the existing guard’s inline-Python shell convention. The shell regression command passed all six tests (exit 0, `/d0/ryan/sunfixamp-model-neutrality/regressions-shell.log`). `scripts/check-no-python-assets.sh`, ShellCheck on the regression shell, and staged pre-commit all exited 0 (`python-assets-final.log`, `shell-regressions-check.log`, `precommit-shell.log` in the same evidence directory). The guide now names that entry point; the earlier Python command above is historical evidence.

## Canonical inventory

“Enumerated” records filesystem coverage, not a semantic neutrality pass.

| Canonical skill path | Coverage | Mirror |
| --- | --- | --- |
| agent-generator-tutor | Enumerated | No |
| agentic-workflow-first | Enumerated | No |
| amplihack-expert | Enumerated | No |
| anthropologist-analyst | Enumerated | Yes |
| aspire | Enumerated | No |
| authenticated-web-scraper | Enumerated | No |
| auto-drive-to-merge | Enumerated | Yes |
| awesome-copilot-sync | Enumerated | No |
| azure-admin | Enumerated | Yes |
| azure-devops | Enumerated | No |
| backlog-curator | Enumerated | Yes |
| biologist-analyst | Enumerated | Yes |
| cascade-workflow | Enumerated | Yes |
| chemist-analyst | Enumerated | Yes |
| claude-agent-sdk | Enumerated | Yes |
| code-atlas | Enumerated | No |
| code-philosophy | Enumerated | No |
| code-smell-detector | Enumerated | Yes |
| code-visualizer | Enumerated | Yes |
| collaboration/creating-pull-requests | Enumerated | Yes |
| computer-scientist-analyst | Enumerated | Yes |
| consensus-voting | Enumerated | Yes |
| context-management | Enumerated | No |
| crusty-old-engineer | Enumerated | Yes |
| cybersecurity-analyst | Enumerated | Yes |
| debate-workflow | Enumerated | Yes |
| default-workflow | Enumerated | Yes |
| dependency-resolver | Enumerated | No |
| design-patterns-expert | Enumerated | Yes |
| dev-orchestrator | Enumerated | No |
| development/architecting-solutions | Enumerated | Yes |
| development/setting-up-projects | Enumerated | Yes |
| documentation-writing | Enumerated | Yes |
| docx | Enumerated | Yes |
| dotnet-exception-handling | Enumerated | No |
| dotnet-install | Enumerated | No |
| dotnet10-pack-tool | Enumerated | No |
| dynamic-debugger | Enumerated | Yes |
| e2e-outside-in-test-generator | Enumerated | No |
| economist-analyst | Enumerated | Yes |
| email-drafter | Enumerated | Yes |
| engineer-analyst | Enumerated | Yes |
| environmentalist-analyst | Enumerated | Yes |
| epidemiologist-analyst | Enumerated | Yes |
| ethicist-analyst | Enumerated | Yes |
| eval-recipes-runner | Enumerated | Yes |
| fleet | Enumerated | No |
| fleet-copilot | Enumerated | No |
| futurist-analyst | Enumerated | Yes |
| gh-aw-adoption | Enumerated | No |
| gh-work-report | Enumerated | No |
| gherkin-expert | Enumerated | Yes |
| github | Enumerated | No |
| github-copilot-cli | Enumerated | No |
| github-copilot-cli-expert | Enumerated | No |
| github-copilot-sdk | Enumerated | No |
| goal-seeking-agent-pattern | Enumerated | Yes |
| historian-analyst | Enumerated | Yes |
| indigenous-leader-analyst | Enumerated | Yes |
| investigation-workflow | Enumerated | Yes |
| journalist-analyst | Enumerated | Yes |
| knowledge-extractor | Enumerated | Yes |
| lawyer-analyst | Enumerated | Yes |
| learning-path-builder | Enumerated | Yes |
| lsp-setup | Enumerated | No |
| markitdown | Enumerated | No |
| mcp-manager | Enumerated | Yes |
| meeting-synthesizer | Enumerated | Yes |
| merge-ready | Enumerated | No |
| mermaid-diagram-generator | Enumerated | Yes |
| meta-cognitive/analyzing-deeply | Enumerated | Yes |
| microsoft-agent-framework | Enumerated | Yes |
| migrate | Enumerated | No |
| model-evaluation-benchmark | Enumerated | Yes |
| module-spec-generator | Enumerated | Yes |
| multi-repo | Enumerated | No |
| multitask | Enumerated | No |
| n-version-workflow | Enumerated | Yes |
| novelist-analyst | Enumerated | Yes |
| npe-hunting-workflow | Enumerated | No |
| outside-in-testing | Enumerated | Yes |
| oxidizer-workflow | Enumerated | No |
| pdf | Enumerated | Yes |
| philosopher-analyst | Enumerated | Yes |
| philosophy-compliance-workflow | Enumerated | Yes |
| physicist-analyst | Enumerated | Yes |
| pm-architect | Enumerated | Yes |
| poet-analyst | Enumerated | Yes |
| political-scientist-analyst | Enumerated | Yes |
| pptx | Enumerated | Yes |
| pr-guide | Enumerated | No |
| pr-review-assistant | Enumerated | Yes |
| pre-commit-manager | Enumerated | No |
| property-based-testing | Enumerated | No |
| psychologist-analyst | Enumerated | Yes |
| qa-team | Enumerated | Yes |
| quality/reviewing-code | Enumerated | Yes |
| quality/testing-code | Enumerated | Yes |
| quality-audit | Enumerated | Yes |
| remote-work | Enumerated | Yes |
| repository-oom-audit | Enumerated | No |
| research/researching-topics | Enumerated | Yes |
| roadmap-strategist | Enumerated | Yes |
| self-improving-agent-builder | Enumerated | No |
| session-learning | Enumerated | No |
| session-replay | Enumerated | No |
| session-to-agent | Enumerated | No |
| shadow-testing | Enumerated | No |
| signal | Enumerated | No |
| signal-setup | Enumerated | No |
| silent-degradation-audit | Enumerated | No |
| skill-builder | Enumerated | Yes |
| smart-test | Enumerated | No |
| sociologist-analyst | Enumerated | Yes |
| socratic-review | Enumerated | No |
| statler-waldorf | Enumerated | No |
| storytelling-synthesizer | Enumerated | Yes |
| supply-chain-audit | Enumerated | No |
| test-gap-analyzer | Enumerated | Yes |
| tla-plus-expert | Enumerated | Yes |
| transcript-viewer | Enumerated | Yes |
| ultrathink-orchestrator | Enumerated | Yes |
| urban-planner-analyst | Enumerated | Yes |
| verus-expert | Enumerated | Yes |
| work-delegator | Enumerated | Yes |
| work-iq | Enumerated | No |
| workflow-enforcement | Enumerated | No |
| workiq-wsl | Enumerated | No |
| workstream-coordinator | Enumerated | Yes |
| xlsx | Enumerated | Yes |

The follow-up replaces the regression heredoc with native Bash fixtures rather
than embedding the former Python test asset. The same six integration scenarios
remain, including seven provider recommendation inputs; fixtures are isolated
under TMPDIR. The production audit still uses its existing inline Python detector.
Both no-Python gates and ShellCheck pass. Supporting YAML now has a Rust corpus
test which parses every YAML document without executing example commands.

Follow-up local results: production catalog acceptance passes for 130 identities,
canonical paths and nonempty bodies, including seven nested skills. Rust parses
all 24 supporting YAML files and every document, with zero exclusions; a malformed
second-document regression also passes. Isolated real-corpus publication verifies
all 130 published SKILL.md files byte-for-byte. The issue #1438 local installation
fixture and issue #1277 nested support-file fixture pass. Frontmatter name/type
checks pass 15 tests, citation checks pass six, and issue #962 passes six checks.
Logs use the `native-` prefix under `/d0/ryan/sunfixamp-model-neutrality`;
`native-review.md` records review and double-check findings. Broad validation of
all referenced assets and provider SDK execution remain unverified.
