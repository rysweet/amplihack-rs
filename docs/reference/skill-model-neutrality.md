---
title: Skill model neutrality
last_updated: 2026-10-04
review_schedule: quarterly
---

# Skill model neutrality

Skills inherit model selection from the user's runtime configuration. They describe required capabilities, such as tool use, structured output, or image input, without prescribing a concrete execution model. This contract covers all 130 canonical skills, nested skills, supporting examples, templates, configuration, scripts, and corresponding documentation mirrors.

For setup and validation commands, see [Configure and validate neutral skills](../howto/skill-model-neutrality.md).

## Execution selection

Canonical `SKILL.md` frontmatter has no `model` override. Instructions, custom-agent configuration, command examples, and runnable scripts contain no hardcoded model defaults, fallback models, or recommendations. Optional SDK model arguments are omitted where the runtime supports inheritance. Required API fields receive user-supplied configuration.

Capability guidance remains specific: an image-description example requires vision support; a tool-calling agent requires tool support. Model choice does not change tool permissions or authorize switching providers.

## Configuration contract

| Setting or notation | Meaning |
| --- | --- |
| Runtime model configuration | Existing runtime choice inherited when the SDK permits omission |
| `AGENT_MODEL` | Required provider model identifier for standalone C# examples `04-basic-agent.cs`, `05-tool-integration.cs`, and `06-simple-workflow.cs` |
| `<configured-model>` | Documentation substitution marker for a user-selected model |
| `<configured-primary-model>` | Independently supplied primary model |
| `<configured-fallback-model>` | Independently supplied fallback model |
| `<configured-deployment-name>` | User's Azure deployment name, rather than the underlying model identifier |
| `AZURE_OPENAI_DEPLOYMENT` | Deployment configuration used by Azure examples that expose this variable |

Angle-bracket markers are documentation placeholders and are never valid executable defaults. Snippets containing them require substitution before execution. Standalone examples obtain required values from environment variables or established runtime configuration. The three C# examples and their mirrors validate `AGENT_MODEL` before constructing clients.

The configuration contract requires rejection of absent, empty, whitespace-only, and unresolved placeholder values before client construction. The three C# examples and their mirrors use `string.IsNullOrWhiteSpace`, reject angle-bracket delimiters, and trim accepted values. Static regression cases cover the validation contract. The validation error names the setting without printing credentials, prompts, or provider responses, and does not fall back to a named model.

Primary and fallback settings are separate configuration inputs; they may intentionally identify the same model, but one is not silently copied into the other. Each provider keeps its own credentials and endpoint configuration. Provider changes require explicit user configuration because they can change data destinations.

## Provider API boundaries

Neutrality preserves SDK parameter names, authentication, endpoints, and provider-specific setup. Fields such as `model` and `llm_model` remain where the API requires them. Azure calls receive the deployment name in the field expected by that client. MarkItDown image examples require a deployment or model with image-input support.

A provider-specific example does not imply that another provider implements the same protocol. Replacing a model identifier does not establish endpoint or SDK compatibility. Credentials come from environment variables or established credential stores; configuration is passed as SDK parameters rather than interpolated into shell commands.

There is no new provider API or model-selection service. These rules govern existing skill content and examples.

## Preserved contextual references

| Context | Why it remains |
| --- | --- |
| GPT-3/4 in the computer-scientist quiz's historical-grounding rubric | Evaluates research knowledge; does not select an execution model |
| GPT progression, Gato, and generic GPT/Claude/LLaMA research examples | Describes research history or model families |
| Sonnet and haiku in the poet skill | Names poetic forms |
| `GEMINI.md` | Names a provider instruction file |
| Provider-specific API terminology and identifiers in reviewed illustrations | Explains an API contract without recommending the skill's execution model |

Concrete contextual identifiers use narrow, reviewed exceptions when the detector would otherwise reject them. Each exception identifies the relative path and exact line text and gives a reason; diagnostics report actual line numbers. No entire file is exempt. Moving an exception to another path, changing its text, or appending execution advice requires review and fails the existing exception match.

## Static guard contract

`amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh` resolves the repository relative to its own location and takes no arguments. Bash and Python 3 are required. It reads repository content as text, imports no provider SDK, executes no scanned snippets, and makes no API requests.

The guard recursively scans `amplifier-bundle/skills` and `docs/claude/skills`, including supporting text files. It enforces the reviewed count and exact inventory of 130 canonical `SKILL.md` files against the audit table and checks canonical frontmatter. Mirror files are scanned for execution selections without requiring unrelated content to be byte-identical.

Versioned model identifiers are rejected unless covered by an exact contextual exception. Unversioned aliases are rejected in assignments, CLI model switches, and execution-selection prose. Regression cases cover quoted keys, model families and aliases, defaults, recommendations, commands, historical exceptions, poetry, and provider API illustrations. Configuration regressions check absent, empty, whitespace-only, placeholder, and valid values through a restricted static source contract without constructing provider clients. Integration fixtures verify diagnostics, inventory drift, frontmatter overrides, and removal of configuration validation. These checks do not establish SDK runtime compatibility.

Success returns zero and reports skill and text-file counts plus detector regression results. A neutrality violation returns nonzero and reports the repository-relative file, line number, and violation. Missing trees, invalid canonical frontmatter, and inventory mismatches also fail. Missing Bash or Python prevents validation and must be reported as a tooling blocker.

Directory symlinks are not recursively followed, avoiding duplicate coverage and cycles. Accessible file links are read; shared linked directories are covered through their canonical targets. Binary content is excluded from text scanning. Broken links and unreadable assets are recorded in the audit rather than reported as audited content.

The reviewed inventory has 123 top-level and seven nested skills, 537 accessible canonical text paths, and 323 mirror text paths containing 81 mirrored skills. Shared README links count as text paths. Two `outside-in-testing` links, `scripts` and `tests`, target missing `qa-team` directories and have no accessible contents to scan.

## Maintaining coverage

New skills or model naming conventions require inventory and detector review. A passing regex guard does not establish semantic neutrality for unknown names or indirect configuration. Manual review includes every supporting file, exact contextual exceptions, provider contracts, and corresponding mirror guidance.

The audit records the full skill inventory, accessible paths, link handling, preserved references and their reasons, validation evidence, tooling blockers, and staged and unstaged state. Static syntax checks are reported separately from SDK compatibility and live runtime validation. Existing unrelated differences between mirrors are preserved.

## Catalog API

The copied baseline loader visited direct children and skipped parse failures; recursive fail-explicit loading and structured metadata support were pending implementation at the original documentation review. This branch now implements them, with production-loader and unit evidence in the [audit](../audits/skill-model-neutrality-documentation.md). The broader structure, supporting-YAML, and installation corpus checks below remain planned unless explicitly recorded as passing.

`amplihack_domain_agents::skill_catalog::SkillCatalog` exposes the existing
`load(skills_dir: &Path) -> Result<Self>` entry point. Loading succeeds only when
every discovered skill is valid. Callers propagate the error or display it with
its source path; they do not continue with a partial catalog.

Discovery visits ordinary directories recursively in sorted path order, including
subdirectories below a directory that already contains a skill. Directories
without `SKILL.md` are containers, rather than invalid skills. Directory symlinks
are skipped. A symlink named `SKILL.md` is rejected before its target is read.
Missing roots, directory-entry and traversal errors, unreadable skill files,
malformed metadata, and empty prompt bodies return path-specific errors. Duplicate
frontmatter names fail with both source paths; one skill never replaces another.

| API | Result |
| --- | --- |
| `len()` / `is_empty()` | Size of the completely loaded catalog |
| `get(name)` | Skill with that frontmatter identity, or `None` |
| `names()` | All identities sorted alphabetically |
| `iter()` | All identity/skill pairs; iteration order is unspecified |
| `match_auto_activate(text)` | Skills with case-insensitive substring matches in `auto_activates` |
| `find_by_trigger(trigger)` | Skill with an exact `explicit_triggers` match, or `None` |

Each `Skill` retains typed `meta`, a nonempty Markdown `prompt`, and the source
skill directory in `path`. Names come from frontmatter, not relative directory
paths: `quality/reviewing-code` resolves as `reviewing-code`, and the legacy
`migrate` directory resolves as `amplihack-migrate`. All other bundled names match
the leaf directory. Resolving a loaded path identifies its canonical directory.
Loading and trigger matching do not execute prompts, grant tool permissions, or
independently authorize external actions. Confirmation flags remain typed and
continue to govern the existing invocation policy.

### Frontmatter and metadata

A skill starts at the first byte with a complete `---` delimiter line, contains a
YAML mapping, and closes frontmatter with another complete `---` line. LF and
CRLF line endings are accepted. A delimiter prefix such as `---invalid` does not
open or close frontmatter. The remaining Markdown body contains non-whitespace
prompt content.

| Field | Accepted contract |
| --- | --- |
| `name` | Required string matching `^[a-z0-9]+(-[a-z0-9]+)*$`; unique across the catalog |
| `description` | Optional string at the API level; absent or null loads as `None`. All 130 bundled skills supply a string; this corpus convention does not make it required for external skills. |
| `version` | Optional string; quote numeric-looking versions |
| `argument-hint` | Optional extension YAML; existing string and sequence forms are preserved |
| `auto_activates`, `explicit_triggers` | Lists of strings; absent lists default to empty |
| `confirmation_required`, `skip_confirmation_if_explicit` | Booleans; absent flags default to false |
| `token_budget` | Optional unsigned 32-bit integer or the supported resource-budget mapping below |

A structured budget has optional unsigned 32-bit fields `skill_md`,
`reference_md`, `examples_md`, `patterns_md`, and `total`. `total` remains optional:
resource budgets without a total are valid. Negative, fractional, overflowing,
and string-valued budgets are rejected. `argument-hint` is retained as extension YAML, so both existing string and sequence forms survive loading. It is not interpreted by this catalog. Supported metadata variants retain their
values through loading and serialization; compatibility does not replace typed
operational fields with unrestricted YAML. Additional descriptive metadata cannot
override operational fields or relax their validation.

Extension fields are stored in `SkillMeta.extensions` and serialized back as frontmatter keys. They preserve YAML values without activating additional runtime behavior. Observed corpus extensions: `activationContextWindow`, `activationKeywords`, `activationStrategy`, `activation_conditions`, `activation_keywords`, `activation_triggers`, `agent`, `allowed-tools`, `amplifier_bundle`, `argument-hint`, `author`, `auto-activation`, `auto-detection`, `auto_activate`, `auto_activate_keywords`, `auto_triggers`, `category`, `complexity`, `dependencies`, `deprecated`, `deprecated_since`, `disable-model-invocation`, `disableModelInvocation`, `disclosure_strategy`, `embedded_framework_version`, `evaluation_criteria`, `github_repo`, `implementation_status`, `invocable_by`, `invokes`, `issue`, `last_updated`, `license`, `maturity`, `maturity_reason`, `max_tokens`, `mcp_server`, `metadata`, `min_tokens`, `output_location`, `persistenceThreshold`, `philosophy`, `planned_languages`, `priority`, `priority_score`, `recipe`, `references`, `related_agents`, `related_files`, `replaced_by`, `requires`, `resource_requirements`, `source_urls`, `supported_languages`, `supporting_docs`, `tags`, `target-agents`, `tools_required`, `triggers`, `type`, `user-invocable`.

```yaml
---
name: sample-neutral-skill
description: Review a change using the configured runtime model
version: "1.0.0"
argument-hint: "[path]"
auto_activates:
  - review this change
explicit_triggers:
  - /amplihack:sample-neutral-skill
confirmation_required: true
skip_confirmation_if_explicit: false
token_budget:
  skill_md: 800
  reference_md: 1200
  examples_md: 600
  total: 2600
---
```

Place a Markdown prompt after this frontmatter. The example is a template for a
new skill, not an additional member of the reviewed 130-skill inventory.

## Complete corpus acceptance

The canonical inventory contains exactly 130 skill paths and 130 unique names:
123 top-level skills and these seven nested skills:

| Relative directory | Frontmatter identity |
| --- | --- |
| `collaboration/creating-pull-requests` | `creating-pull-requests` |
| `development/architecting-solutions` | `architecting-solutions` |
| `development/setting-up-projects` | `setting-up-projects` |
| `meta-cognitive/analyzing-deeply` | `analyzing-deeply` |
| `quality/reviewing-code` | `reviewing-code` |
| `quality/testing-code` | `testing-code` |
| `research/researching-topics` | `researching-topics` |

Acceptance compares an independent recursive inventory with actual production
loader results by exact names and resolved source directories. It also checks
all 130 prompt bodies. Equal counts alone cannot detect a substituted or omitted
skill. A missing corpus fails acceptance rather than skipping the test.

Planned `skill_corpus_validation` coverage (no registered target yet) will apply repository contracts: uppercase `SKILL.md`, valid
frontmatter, nonempty prompts, documented usage and instructions, and required
assets. It recognizes existing section conventions rather than imposing one
new heading template on all skills. Supporting `.yaml` and `.yml` files are
parsed as data, including every document in multi-document files, using the
repository's Rust YAML dependency. Non-standalone templates have individually
recorded path/reason exclusions; exclusions never cover malformed skill
frontmatter. No embedded example commands are executed by these checks.

Planned complete-corpus installation acceptance uses isolated fixtures and verifies canonical and nested
skills reach the published locations without modifying live installations.
Mirror acceptance retains the issue #849 citation policy and issue #962
byte-equality contract where applicable, alongside semantic review of changed
skill documentation. A neutrality scan does not substitute for these checks.

Validation evidence identifies the tested revision, command, exit code, exact
coverage, YAML counts and exclusions, review findings, and unavailable tooling.
YAML validity, production loading, installation, and SDK compatibility are
separate results. Neither parsing nor loading proves SDK compilation, provider
availability, or successful execution of examples.
