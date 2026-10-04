# Configure and validate neutral skills

Use your runtime's configured model when invoking a skill. Choose a model that meets the skill's capability requirements and is available through your configured provider. See the [configuration and guard reference](../reference/skill-model-neutrality.md) for the complete contract.

## Configure standalone examples

The Microsoft Agent Framework C# examples require `AGENT_MODEL`. Set it to an available model through your environment or credential-management workflow before running the example. Keep provider credentials and endpoints configured as described in the provider's skill documentation.

For documentation snippets, replace `<configured-model>` with that value. For Azure snippets, replace `<configured-deployment-name>` with your Azure deployment name. Image examples require image-input support. Configure primary and fallback values separately when an example uses both.

Do not execute snippets with unresolved angle-bracket placeholders. The three C# examples and their mirrors reject absent, empty, whitespace-only, and angle-bracket placeholder `AGENT_MODEL` values before client construction; valid values are trimmed. If validation fails, correct the named setting and retry; model selection never supplies missing credentials or changes tool permissions.

## Check a skill change

1. Review the skill, its nested skills, examples, templates, scripts, and configuration. Replace execution-model recommendations with capability requirements. Remove optional model overrides where the runtime supports defaults; supply explicit required API values from user configuration.
2. Apply equivalent guidance to corresponding files in `docs/claude/skills`. Preserve unrelated mirror differences.
3. Retain historical research and poetic terms. For concrete provider API or historical identifiers flagged by the guard, document the reason and review an exact path-and-line exception. Never exclude an entire file.
4. Run the static checks below. Review the full diff and audit inventory before accepting a change to the fixed skill count.

Run from the repository root:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
export CARGO_TARGET_DIR=/d0/ryan/sunfixamp-model-neutrality/cargo-target
export TMPDIR=/d0/ryan/sunfixamp-model-neutrality/tmp
mkdir -p "$TMPDIR"
bash amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh
bash -n amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh
bash amplifier-bundle/recipes/tests/test-issue-962-skill-mirror-parity.sh
git diff HEAD --check
```

The neutrality guard reports 130 canonical skills on success; text-file counts reflect the scanned inventory. The mirror check verifies the existing default-workflow parity contract. To verify caller-directory independence, invoke the guard by its absolute path from an unrelated directory under `/d0`.

When ShellCheck is installed, also run:

```bash
shellcheck amplifier-bundle/recipes/tests/test-skill-model-neutrality.sh
```

These checks need no build or live API access. Keep any temporary files, caches, and build output under `/d0`. Parse changed snippets with available static tools and test required configuration rejection without making provider requests. Report missing SDKs, C# tooling, project manifests, and credentials separately; syntax validation alone does not prove SDK compatibility.

## Resolve guard failures

For a file-and-line violation, inspect whether the reference selects an execution model. Remove a pin or recommendation, or review a contextual exception with its reason. A historical line followed by execution advice is not a valid exception.

For an inventory mismatch, enumerate all canonical `SKILL.md` files, including nested skills, and review additions or removals before updating the expected count. Record broken or inaccessible linked assets. Do not describe them as covered by a passing scan.

Record commands, outcomes, blockers, preserved contextual references, and final staged and unstaged status in the audit. Preserve prior work and record review findings with the tested revision.

## Load and resolve a nested skill

At the original review, recursive fail-explicit loading and metadata compatibility were pending implementation. This branch implements those changes; see the audit for actual loader evidence. Supporting YAML has corpus validation; isolated publication now covers all 130 skills. Broader referenced-asset validation remains planned.

Pass the canonical skills root to the production catalog. Propagate loading
failures so the caller sees the source path instead of an incomplete catalog:

```rust
use std::path::Path;
use amplihack_domain_agents::skill_catalog::SkillCatalog;

let catalog = SkillCatalog::load(Path::new("amplifier-bundle/skills"))?;
let skill = catalog.get("reviewing-code").expect("bundled reviewing-code skill");
assert!(skill.path.ends_with("quality/reviewing-code"));
assert!(!skill.prompt.trim().is_empty());
```

This fragment belongs in a function returning a compatible `Result`. Lookup
returns the skill data for the existing invocation flow. Apply that flow's
confirmation and authorization checks before executing instructions.

When adding metadata, keep strings, lists, booleans, and budgets in the forms
listed in the [catalog reference](../reference/skill-model-neutrality.md#catalog-api).
Quote numeric-looking string values. Keep structured resource budgets intact;
converting them to a scalar loses disclosure information. Add a Markdown body
that explains purpose, usage, and instructions, and include referenced required
assets. Run both metadata checks and the production-loader acceptance tests.

## Validate complete loading and publication

Use the environment exports above for every build-capable command. Keep Python
bytecode and pre-commit caches under the evidence root as well:

```bash
export PYTHONDONTWRITEBYTECODE=1
export PRE_COMMIT_HOME=/d0/ryan/sunfixamp-model-neutrality/pre-commit
export XDG_CACHE_HOME=/d0/ryan/sunfixamp-model-neutrality/cache
```

Registered acceptance commands are shown below. Cargo target names come from `bins/amplihack/Cargo.toml`, rather than guessing from filenames: `skill_frontmatter_name_test.rs`, `skill_frontmatter_type_test.rs`, and `issue_849_skill_mirror_citation_test.rs` are registered without the `_test` suffix. `skill_corpus_validation` is an automatically discovered integration test in `amplihack-domain-agents` for supporting YAML documents.

```bash
cargo test -p amplihack-domain-agents --test bundled_skill_catalog
cargo test -p amplihack-domain-agents skill_catalog
cargo test -p amplihack --test skill_frontmatter_name --test skill_frontmatter_type
cargo test -p amplihack --test issue_849_skill_mirror_citation
cargo test -p amplihack-cli issue_1438_skill_publication
cargo test -p amplihack-cli real_corpus_publishes_all_130_skills_in_isolated_home
cargo test -p amplihack-cli issue_1277_stages_top_level_and_nested_skills_with_all_support_files
bash amplifier-bundle/recipes/tests/test-skill-model-neutrality-regressions.sh
pre-commit run --all-files
```

Inspect existing logs before repeating expensive checks. Logs from an earlier
revision are baseline evidence; validate affected contracts again after
reconciliation or fixes. Record unavailable tests explicitly instead of treating
an absent test target as a pass.

For this workstream, record evidence under
`/d0/ryan/sunfixamp-model-neutrality`: reconciliation manifest, validation
commands and exit codes, supporting-YAML counts and individual exclusions, and
review findings. A successful loading result accounts for all 130 names and
paths, all seven nested skills, and nonempty bodies. Preserve original workspace
edits and both pre-reconciliation patches before consolidating changes in the
single development branch.

After validation, complete the default-workflow review and double-check, resolve
findings or document limitations, then commit, push, and create the PR. Review the
published diff and run `gh pr checks --watch`. Record the PR URL and final branch
revision in the evidence. Keep the PR open and unmerged for user review. These
publication steps apply to the complete implementation workflow; documentation
specification and historical logs do not establish their completion.

## Troubleshoot catalog failures

- For a malformed frontmatter error, inspect the named file's complete delimiter
  lines, mapping shape, field types, and prompt body. Fix the source rather than
  dropping it from the inventory.
- For duplicate names, inspect both paths reported by the loader. Assign the
  intended unique identity and update its directory or documented legacy mapping.
- For traversal errors, restore access to the named directory. An inaccessible
  subtree prevents a complete catalog.
- For a symlinked `SKILL.md`, place the canonical skill in an ordinary directory.
  Shared supporting assets can retain their separately audited link policy.
- For a coverage mismatch, compare exact inventory identities and paths. Check
  nested directories and typed metadata before changing the expected count.
