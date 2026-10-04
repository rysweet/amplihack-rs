# Skill model-neutrality completion audit

Audit date: 2026-10-04. Scope: all 130 canonical skills (123 top-level and
seven nested), all 537 existing canonical file paths, and all 323 existing
documentation mirror file paths (81 mirrored SKILL.md files). The scanner reads
860 text paths, including a shared README symlink; linked directories resolve
into already scanned `common/ooxml` and `qa-team` trees.

## Earlier worktree implementation

The worktree and index statements in this section record the earlier implementation step; see the final original-workspace review below.

### Existing work and final changes

Reviewed and retained the existing worktree edits (70 tracked modified files plus
new audit, guard, and documentation files). The worktree index was empty and
remains unchanged. The parent repository staged implementation was captured
separately and verified byte-for-byte unchanged after this step. One sequential workstream was used.
No commits, PRs, publication, merging, issues, or external messages were performed.

Existing changes replace execution model identities with runtime configuration,
remove benchmark defaults and custom-agent pins, correct the tokenizer example,
and label configuration placeholders. Provider SDKs, credentials, endpoints,
and API model fields remain in their original provider context.

Additional corrections:

- Removed the remaining `{opus|sonnet}` benchmark command from the documentation
  mirror and matched the canonical manual execution guidance. The old Python
  runner is not shipped in this repository.
- Made the local fallback model placeholder explicit in both framework references.
- Corrected Azure MarkItDown configuration to specify an Azure deployment name;
  added missing imports to Azure and Anthropic compatibility snippets. Their
  Python syntax parses. Removed the claim that placeholder examples are ready
  to run without configuration.
- Expanded the neutrality guard to scan documentation mirrors, unversioned aliases,
  quoted JSON/Python model keys, commands, recommendations, and additional model
  families. It enforces the 130-skill inventory and tests rejection/acceptance cases.

## Contextual references and policy

The computer-scientist quiz's GPT-3/4 historical research rubric is retained via
an exact path-and-line exception with a documented reason. Appending execution
instructions or moving that line to another file fails the regression cases.
The same reviewed line in its documentation mirror is permitted.

The poet skill's sonnet and haiku terminology describes poetic forms and is
retained. Generic GPT/Claude/LLaMA examples in the computer-scientist skill and
GPT progression/Gato in the futurist quiz describe research. GEMINI.md denotes
a provider-specific instruction filename. None selects the skill execution model.

Provider examples retain API fields and provider-specific setup, with clearly
marked model placeholders. Azure deployment names are distinguished from model
IDs. Standalone C# examples read AGENT_MODEL and reject absent, empty, whitespace-only,
and angle-bracket placeholder values before constructing clients; valid values
are trimmed;
the standalone Copilot TypeScript example inherits its runtime default.
The detector permits additional reviewed historical or provider API identifiers
through narrow exact-line exceptions with reasons; it does not exclude whole
files. A synthetic provider API exception is exercised by its regression cases.
This is a static guard for known model families, not a semantic proof for every
possible future model name; new names/context require review and detector updates.

## TDD follow-up in the target worktree

Added detector regressions first: `Use Claude for this skill` failed initially.
After narrowing generic Claude/GPT detection to model-selection contexts (so
CLI names, SDK names, and CLAUDE.md paths remain legitimate), the new C# source
contract failed on missing blank-value rejection. Hardened all three examples
and their mirrors, then reran the checks successfully. The source contracts
cover absent, empty, whitespace-only, unresolved angle-bracket placeholders,
and valid configured values without SDK imports or API calls. These are static
checks of a restricted expression, not compiled C# execution.

Added `test_skill_model_neutrality.py`: six offline integration tests exercise
an unrelated caller directory, supporting-file and mirror pin diagnostics,
frontmatter overrides and inventory drift, and removed configuration validation.
Follow-up review added failing regressions for generic provider recommendations,
`default_model` assignments, and replacing a skill while retaining the count of
130. The guard now checks the exact reviewed inventory against this table and
detects those selection forms. All six integration tests passed after correction.
The parent staged patch was snapshotted under `/d0` and compared unchanged.
Fixtures respect the caller’s TMPDIR; this run uses `/d0/ryan/sunfixamp-model-neutrality/tmp`.
The guard, Bash syntax, ShellCheck, six default-workflow mirror assertions,
six integration tests, and `git diff HEAD --check` passed in this worktree.
All three C# mirror pairs are identical. Reviewed the 17 remaining family-name
lines: poetic forms, historical research, generic research examples, and the
provider instruction filename `GEMINI.md`; no execution selection remains.
Updated all three model-neutrality documentation pages to describe implemented
validation. No builds were needed. Preserved NODE_OPTIONS=--max-old-space-size=32768,
CARGO_TARGET_DIR=/d0/ryan/sunfixamp-model-neutrality/cargo-target, and TMPDIR above.

Final changes are unstaged and uncommitted in
`/home/ryan/src/SunFixAmp/worktrees/feat/issue-1532-complete-model`.
The parent `/home/ryan/src/SunFixAmp` staged diff is unchanged.

## Validation and limitations

Original implementation evidence (retained for context; the rerun results are above):

- `bash amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh` from the
  repository and from `/d0`: 130 skills, 860 text files, all detector regression cases.
- `bash amplifier-bundle/recipes/tests/test-issue-962-skill-mirror-parity.sh`:
  six assertions passed.
- Bash syntax and ShellCheck for the neutrality script.
- Model-selection guidance agrees in all 25 initially changed canonical files
  that have corresponding documentation mirrors.
- System Python/PyYAML parses the custom comprehension YAML and confirms no model pin.
- Python AST parsing for the changed Azure/custom-provider snippets.
- Final diff review and `git diff HEAD --check` for staged and unstaged edits.

Live provider examples were not executed: this environment lacks openai,
markitdown, tiktoken, claude_agents, and copilot packages and configured API
credentials. C# compilation is unavailable because dotnet and example project
manifests are absent. No packages or API contracts were introduced to bypass this.

Two pre-existing outside-in-testing links (`scripts` and `tests`) target missing
qa-team directories; they contain no accessible supporting files to audit.
Other linked assets are included through their canonical targets. Documentation
mirrors have pre-existing differences, including Python snippets labelled as
Rust and two missing frontmatters; those unrelated differences were preserved.
Canonical frontmatter is checked, and both trees are scanned for execution pins.

## Final original-workspace review

Final changes are held uncommitted in `/home/ryan/src/SunFixAmp`, with the original staged implementation preserved and review corrections unstaged. Fixed detector gaps for unversioned aliases in `primary_model` and `fallback_model` fields and generic Claude/GPT aliases in CLI model switches. Added integration and detector regressions; the new integration case failed before the fix. No builds or provider requests were needed. Temporary files and test fixtures remain under the requested `/d0` TMPDIR.

Final review checks passed: neutrality guard from the root and an unrelated `/d0` directory (130 skills, 860 text paths), all six Python integration tests, Bash syntax, ShellCheck, six mirror-policy assertions, and diff whitespace checks. System Python parsed 97 configured Python snippets and both custom-agent YAML examples; all three C# mirror pairs match. Corrected a nonexistent benchmark runner reference in both skill copies to describe manual execution. The `tsc` launcher points to a missing TypeScript installation, so TypeScript parsing was unavailable; C# compilation and live provider execution remain unverified.

## Complete skill inventory

Every row was included in the canonical frontmatter/model-reference scan.
All supporting files below each skill directory were scanned; shared/group-level
supporting files were also covered by the full-tree traversal. “Mirror” records
whether a corresponding SKILL.md exists, not byte parity of unrelated content.

| Skill path | Audit | Mirror |
| --- | --- | --- |
| agent-generator-tutor | Pass | No |
| agentic-workflow-first | Pass | No |
| amplihack-expert | Pass | No |
| anthropologist-analyst | Pass | Yes |
| aspire | Pass | No |
| authenticated-web-scraper | Pass | No |
| auto-drive-to-merge | Pass | Yes |
| awesome-copilot-sync | Pass | No |
| azure-admin | Pass | Yes |
| azure-devops | Pass | No |
| backlog-curator | Pass | Yes |
| biologist-analyst | Pass | Yes |
| cascade-workflow | Pass | Yes |
| chemist-analyst | Pass | Yes |
| claude-agent-sdk | Pass | Yes |
| code-atlas | Pass | No |
| code-philosophy | Pass | No |
| code-smell-detector | Pass | Yes |
| code-visualizer | Pass | Yes |
| collaboration/creating-pull-requests | Pass | Yes |
| computer-scientist-analyst | Pass | Yes |
| consensus-voting | Pass | Yes |
| context-management | Pass | No |
| crusty-old-engineer | Pass | Yes |
| cybersecurity-analyst | Pass | Yes |
| debate-workflow | Pass | Yes |
| default-workflow | Pass | Yes |
| dependency-resolver | Pass | No |
| design-patterns-expert | Pass | Yes |
| dev-orchestrator | Pass | No |
| development/architecting-solutions | Pass | Yes |
| development/setting-up-projects | Pass | Yes |
| documentation-writing | Pass | Yes |
| docx | Pass | Yes |
| dotnet-exception-handling | Pass | No |
| dotnet-install | Pass | No |
| dotnet10-pack-tool | Pass | No |
| dynamic-debugger | Pass | Yes |
| e2e-outside-in-test-generator | Pass | No |
| economist-analyst | Pass | Yes |
| email-drafter | Pass | Yes |
| engineer-analyst | Pass | Yes |
| environmentalist-analyst | Pass | Yes |
| epidemiologist-analyst | Pass | Yes |
| ethicist-analyst | Pass | Yes |
| eval-recipes-runner | Pass | Yes |
| fleet | Pass | No |
| fleet-copilot | Pass | No |
| futurist-analyst | Pass | Yes |
| gh-aw-adoption | Pass | No |
| gh-work-report | Pass | No |
| gherkin-expert | Pass | Yes |
| github | Pass | No |
| github-copilot-cli | Pass | No |
| github-copilot-cli-expert | Pass | No |
| github-copilot-sdk | Pass | No |
| goal-seeking-agent-pattern | Pass | Yes |
| historian-analyst | Pass | Yes |
| indigenous-leader-analyst | Pass | Yes |
| investigation-workflow | Pass | Yes |
| journalist-analyst | Pass | Yes |
| knowledge-extractor | Pass | Yes |
| lawyer-analyst | Pass | Yes |
| learning-path-builder | Pass | Yes |
| lsp-setup | Pass | No |
| markitdown | Pass | No |
| mcp-manager | Pass | Yes |
| meeting-synthesizer | Pass | Yes |
| merge-ready | Pass | No |
| mermaid-diagram-generator | Pass | Yes |
| meta-cognitive/analyzing-deeply | Pass | Yes |
| microsoft-agent-framework | Pass | Yes |
| migrate | Pass | No |
| model-evaluation-benchmark | Pass | Yes |
| module-spec-generator | Pass | Yes |
| multi-repo | Pass | No |
| multitask | Pass | No |
| n-version-workflow | Pass | Yes |
| novelist-analyst | Pass | Yes |
| npe-hunting-workflow | Pass | No |
| outside-in-testing | Pass | Yes |
| oxidizer-workflow | Pass | No |
| pdf | Pass | Yes |
| philosopher-analyst | Pass | Yes |
| philosophy-compliance-workflow | Pass | Yes |
| physicist-analyst | Pass | Yes |
| pm-architect | Pass | Yes |
| poet-analyst | Pass | Yes |
| political-scientist-analyst | Pass | Yes |
| pptx | Pass | Yes |
| pr-guide | Pass | No |
| pr-review-assistant | Pass | Yes |
| pre-commit-manager | Pass | No |
| property-based-testing | Pass | No |
| psychologist-analyst | Pass | Yes |
| qa-team | Pass | Yes |
| quality/reviewing-code | Pass | Yes |
| quality/testing-code | Pass | Yes |
| quality-audit | Pass | Yes |
| remote-work | Pass | Yes |
| repository-oom-audit | Pass | No |
| research/researching-topics | Pass | Yes |
| roadmap-strategist | Pass | Yes |
| self-improving-agent-builder | Pass | No |
| session-learning | Pass | No |
| session-replay | Pass | No |
| session-to-agent | Pass | No |
| shadow-testing | Pass | No |
| signal | Pass | No |
| signal-setup | Pass | No |
| silent-degradation-audit | Pass | No |
| skill-builder | Pass | Yes |
| smart-test | Pass | No |
| sociologist-analyst | Pass | Yes |
| socratic-review | Pass | No |
| statler-waldorf | Pass | No |
| storytelling-synthesizer | Pass | Yes |
| supply-chain-audit | Pass | No |
| test-gap-analyzer | Pass | Yes |
| tla-plus-expert | Pass | Yes |
| transcript-viewer | Pass | Yes |
| ultrathink-orchestrator | Pass | Yes |
| urban-planner-analyst | Pass | Yes |
| verus-expert | Pass | Yes |
| work-delegator | Pass | Yes |
| work-iq | Pass | No |
| workflow-enforcement | Pass | No |
| workiq-wsl | Pass | No |
| workstream-coordinator | Pass | Yes |
| xlsx | Pass | Yes |
