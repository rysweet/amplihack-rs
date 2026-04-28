# Documentation Parity Audit

How the amplihack-rs documentation site reached parity with upstream amplihack, and how to maintain it.

## Status

**Complete** (closed 2026-04-27). All 788 upstream documentation files have been audited and dispositioned (199 ported, 94 already existed, 495 omitted). Zero NEW-PR rows remain — every file has a final disposition. See [issue #420](https://github.com/rysweet/amplihack-rs/issues/420) for the full matrix.

## Disposition Codes

Every upstream file received one of these codes:

| Code | Meaning |
|---|---|
| `PORTED #<pr>` | Content ported to amplihack-rs in the listed PR |
| `EXISTS @ <path>` | Equivalent content already exists at the named path |
| `OMIT-bundle` | Per-agent/per-command content shipped in `amplifier-bundle/`, not web docs |
| `OMIT-atlas` | amplihack-rs has its own atlas pipeline (`docs/atlas/`) |
| `OMIT-changelog` | Release notes live in GitHub Releases, not docs |
| `OMIT-py` | Python-only feature not applicable to the Rust port |
| `OMIT-not-impl` | Feature not yet implemented; follow-up issue filed with `docs-audit-followup` label |

## Wave History

The audit ran across eight logical waves (each encompassing one or more execution passes):

| Wave | Scope | PRs |
|---|---|---|
| 1 | README parity (Core Concepts, Features, Navigator, Windows) | #423 |
| 2 | Curated context (PHILOSOPHY, PATTERNS, TRUST, DEFAULT_WORKFLOW) | #424 |
| 3–5 | Core reference, tutorials, how-to guides | #425–#459 |
| 6 | DDD, Power Steering, Tutorials, Hive Mind, Fleet, Memory, Standalone Features | #460, #461, #462, #464, #466, #468 |
| 7 | CS Validator, Implementation, Investigations, MCP Evaluation, Remote Sessions, Security, Skills, Testing, Troubleshooting | #469–#474 |
| 8 | Final disposition cleanup (reclassified 38 rows; 0 NEW-PR remaining) | — |
| 309 | Verification pass — confirmed 0 NEW-PR rows remain; no-op (all criteria met trivially) | — |
| 317 | Verification pass — re-confirmed 0 NEW-PR rows; issue #420 closed; all 6 acceptance criteria met | — |

## Porting a New Upstream Page

When upstream adds documentation that amplihack-rs should carry:

1. **Check the upstream file.** Determine whether it applies to the Rust port (skip Python-only content).
2. **Pick the right Diataxis type.** Tutorials go in `docs/tutorials/`, how-to guides in `docs/howto/`, reference in `docs/reference/`, concepts in `docs/concepts/`.
3. **Adapt content for Rust.** Replace Python code samples, `uvx`/`pip` commands, and Python-specific tooling with Rust equivalents (`cargo`, `amplihack` CLI commands).
4. **Add a nav entry.** Update the `nav:` section in `mkdocs.yml` under the appropriate heading.
5. **Build and verify.** Run `mkdocs build --strict` to confirm no broken links or missing nav entries.
6. **Open a PR.** Restrict the diff to `docs/**/*.md` and `mkdocs.yml` unless fixing infrastructure bugs.

## Maintaining Parity

### Next Steps (Priority Order)

Future documentation work should follow this order:

1. **Fresh upstream audit** — Compare current upstream `docs/` against the audit matrix to catch files added after Wave 8. This is self-contained and can start immediately.
2. **Documentation quality improvements** — Refine existing ported pages (clarity, Rust-specific examples, cross-linking).
3. **Resolve gating issues** — Port documentation for #433, #434, #435 once the underlying features ship. These are blocked by external implementation dependencies.

### Periodic Checks

Compare `docs/` file counts between upstream and amplihack-rs. Any new upstream pages that aren't `OMIT-*` need porting.

```sh
# Count ported docs
find docs/ -name '*.md' | wc -l

# Check for broken internal links
mkdocs build --strict
```

### What Stays Out

These categories are intentionally omitted and should not be ported:

- **Bundle content** (`OMIT-bundle`): Agent definitions, command files, and tool configs ship inside `amplifier-bundle/` and are not web-facing documentation.
- **Python-only features** (`OMIT-py`): Plugin system (`uvx`), SDK adapters, and Python-specific tooling.
- **Changelog** (`OMIT-changelog`): Version history lives in GitHub Releases.
- **Atlas** (`OMIT-atlas`): amplihack-rs has its own atlas pipeline with different architecture.

### Gated Items

Some upstream features are not yet implemented in amplihack-rs. These are tracked with `OMIT-not-impl` and linked to follow-up issues. When a feature ships, port its documentation and update the disposition.

**Implementation-gated** (feature must ship first):

| Issue | Description |
|---|---|
| [#433](https://github.com/rysweet/amplihack-rs/issues/433) | Implement interactive installation wizard |
| [#434](https://github.com/rysweet/amplihack-rs/issues/434) | Decide disposition of upstream gherkin v2 experiment findings |
| [#435](https://github.com/rysweet/amplihack-rs/issues/435) | Decide disposition of upstream DISCOVERIES log |

**Documentation follow-ups** (filed during audit):

| Issue | Description |
|---|---|
| [#421](https://github.com/rysweet/amplihack-rs/issues/421) | Feature gap: Workflow Execution Guardrails not implemented in amplihack-rs |
| [#422](https://github.com/rysweet/amplihack-rs/issues/422) | Feature gap: Dual-Provider workflow not implemented in amplihack-rs |
| [#475](https://github.com/rysweet/amplihack-rs/issues/475) | Document cs-validator integration once Rust port lands |
| [#476](https://github.com/rysweet/amplihack-rs/issues/476) | Document MCP evaluation user guide once Rust port lands |

## MkDocs Build Verification

Every documentation PR must pass:

```sh
mkdocs build --strict
```

This catches:

- Missing nav entries for new pages
- Broken cross-references between pages
- Orphan pages not linked from navigation
- Invalid Markdown syntax

## Related

- [File Organization](./file-organization.md) — Where files go in the repository
- [Issue #420](https://github.com/rysweet/amplihack-rs/issues/420) — Full audit matrix with per-file dispositions
