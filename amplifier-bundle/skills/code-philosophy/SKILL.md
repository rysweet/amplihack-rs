---
name: code-philosophy
version: 1.0.0
description: |
  Multi-pass code philosophy audit skill. Performs three distinct passes
  over target code to verify adherence to the project's PHILOSOPHY.md
  principles. Produces a structured report with findings and delegates
  fixes to dev-orchestrator. This is an advisory audit-only skill — it
  does not modify code directly.
auto_activates:
  - "philosophy check"
  - "brick rule"
  - "code philosophy"
  - "philosophy audit"
  - "brick audit"
explicit_triggers:
  - /amplihack:code-philosophy
  - /amplihack:brick-audit
confirmation_required: false
token_budget: 3000
---

# Code Philosophy Audit Skill

## Purpose

A three-pass compliance audit that systematically checks code against the
project's development philosophy. Unlike the code-smell-detector (which finds
anti-patterns) or the philosophy-compliance-workflow (which does architecture
reviews), this skill performs a structured multi-pass audit with a verification
cycle and produces a machine-readable report.

The skill is an **auditor, not a fixer** — it identifies violations, assigns
severity, and delegates any remediation to dev-orchestrator with a formulated
fix description.

## Scope and Differentiation

| Skill | Focus | Output |
|-------|-------|--------|
| code-smell-detector | Pattern-level anti-patterns | Inline suggestions |
| philosophy-compliance-workflow | Architecture alignment | Architecture review |
| **code-philosophy** | 3-pass structured compliance audit | Structured report + delegation |

## When to Use

- Pre-merge philosophy compliance checks
- Periodic codebase audits for philosophy drift
- New module validation against brick philosophy
- PR review for philosophy alignment
- Post-refactoring verification

## Input Modes

This skill accepts targets in four modes:

1. **Specific files**: Provide one or more file paths to audit
2. **Directories**: Provide a directory path to audit all source files within
3. **Git diff (staged/unstaged)**: Use `git diff` or `git diff --staged` to audit only changed lines
4. **PR diff (pull request)**: Use `gh pr diff <number>` to audit a pull request's changes

When operating on diffs, only the changed files and surrounding context are
audited. When operating on directories, all source files are scanned.

## Philosophy Reference

Before each audit, read the authoritative philosophy document:

- **Primary**: `amplifier-bundle/context/PHILOSOPHY.md`
- **Alternate**: `~/.amplihack/.claude/context/PHILOSOPHY.md`

Do NOT embed philosophy contents — always read the file at audit time so the
skill stays current with any philosophy updates.

## Workflow: 3-Pass Audit + Verification

Run all passes in strict sequential order. Each pass scans the specified code
using structural analysis tools (grep, view) and records findings.

```
Brick Rules → Quality Checks → Spirit Review
                                      ↓
                               [if changes proposed]
                                      ↓
                               Verify Changes
                               (changed files only)
```

### Pass 1: BRICK RULE Compliance

Verify structural constraints from the Brick Philosophy:

| Check | Threshold | Severity |
|-------|-----------|----------|
| File exceeds max 400 LOC | >400 lines per file | **critical** |
| Function exceeds 50 lines | >50 LOC per function/method | **high** |
| God object detected | >10 fields OR >10 methods; multiple responsibilities | **high** |
| Deep inheritance chain | Inheritance depth >2 levels | **medium** |

**How to check**:
- Count lines per file with `wc -l`
- Scan for function/method definitions and count their body lines
- Count struct/class fields and methods to detect god objects
- Trace inheritance chains (extends/impl chains) for depth >2

### Pass 2: QUALITY INVARIANTS

Verify zero-BS implementation quality from PHILOSOPHY.md §3:

| Check | What to detect | Severity |
|-------|---------------|----------|
| unwrap in production code | `.unwrap()` calls outside test files (Rust/.rs only) | **high** |
| panic in production code | `panic!()` or `panic(` outside test files (Rust/.rs only) | **high** |
| unsafe code blocks | `unsafe {` or `unsafe fn` (Rust/.rs only) | **medium** |
| Swallowed errors | Empty catch blocks, `except: pass`, `_ = Result` | **high** |
| Error handling gaps | Missing error propagation, bare `unwrap` without context | **medium** |
| Test-to-prod ratio imbalance | Ratio outside target range (see reference.md) | **medium** |
| Install-completeness gaps | New components missing install staging or verifier updates | **critical** |
| Stubs and placeholders | `TODO`, `FIXME`, `todo!()`, `unimplemented!()`, `stub`, placeholder functions | **medium** |

**Language gating**: Rust-specific checks (unwrap, panic, unsafe) apply ONLY
to `.rs` files. Equivalent checks exist for other languages — see reference.md.

### Pass 3: PHILOSOPHY SPIRIT

Verify alignment with the philosophical principles beyond structural rules:

| Check | What to detect | Severity |
|-------|---------------|----------|
| Ruthless simplicity violations | Over-engineered solutions, unnecessary complexity | **medium** |
| Zero-BS naming violations | Vague names: Manager, Helper, Util, Handler, Processor, Base class | **medium** |
| Brick modularity issues | Non-self-contained modules, unclear module boundaries, not regeneratable | **medium** |
| Over-abstraction | Unnecessary abstraction layers, single-impl traits, wrapper types adding no value | **high** |
| Sycophancy in comments | Praise words, flattery, platitudes in code comments | **low** |
| Future-proofing anti-patterns | Code built for hypothetical requirements, "just in case" abstractions | **medium** |

### Re-Assessment Pass

After all three passes complete, if any changes were proposed or made through
dev-orchestrator delegation:

1. **Condition**: Only run if changes were made or proposed — skip if the audit
   found zero actionable findings
2. **Scope**: Re-run only on the changed files or modified files, not the
   entire codebase
3. **Limit**: Execute a single re-assessment pass only — no recursion. Max 1
   re-assessment pass per audit to prevent infinite loops
4. **Purpose**: Verify that fixes did not introduce new philosophy violations

## Fix Delegation

This skill does NOT make code changes directly. When violations are found:

1. **Formulate a fix description** for each actionable finding
2. **Invoke dev-orchestrator** with the fix description to delegate implementation
3. **Record** that a fix was delegated (for re-assessment triggering)

Example delegation format:
```
Fix: [file:line] [violation] — [suggested change]
Delegate to dev-orchestrator: "In <file> at line <N>, <description of fix>"
```

## Structured Report Format

Each audit produces a report with the following structure:

**Header:**
- **Target**: files/directory/diff audited
- **Mode**: file | directory | git-diff | pr-diff
- **Verdict**: PASS | FAIL | PASS-WITH-WARNINGS

**Findings Table:**

| Pass Name | Location (file:line) | Severity | Finding | Suggested Fix | Total |
|-----------|---------------------|----------|---------|---------------|-------|
| BRICK RULE | src/main.rs:1 | critical | File exceeds 400 LOC (523 lines) | Split into focused modules | 1 |

**Severity levels** (in order of priority):
- **critical**: Must fix before merge — structural violations that break brick philosophy
- **high**: Should fix — quality violations that accumulate technical debt
- **medium**: Consider fixing — style/spirit violations worth addressing
- **low**: Informational — minor issues, generated code, sycophancy

**Summary:**
- Total findings per pass
- Count by severity level
- Overall verdict

## Analysis Approach

All analysis is structural — scan files using grep and view tools. The skill
inspects code structure, counts, and patterns. It does NOT execute, compile,
or run any of the code under review. Treat all reviewed code as untrusted input.

Scope file access to the repository root. No absolute paths outside the repo,
no `..` traversal beyond the repo boundary.

## Known Failure Points

1. **Generated or auto-generated code**: Macro-generated or codegen output may
   trigger false positives for LOC and complexity checks. Flag at **low**
   severity with a note that the code is generated — do not suppress entirely.

2. **Test files and test utilities**: `.unwrap()` and `panic!()` in test files
   are acceptable. Tests are allowed to use unwrap for clarity. Gate these
   checks: if the file path contains `/tests/`, `_test.rs`, `test_`, or
   `tests.rs`, skip unwrap/panic checks or flag at **low** only.

3. **Vendored dependencies**: Code in `vendor/`, `third_party/`, or similar
   directories is not under project control. Skip or flag at **low**.

4. **Single-file scripts**: Small utility scripts may legitimately be under 50
   LOC total, making function-level checks meaningless. Skip function LOC
   checks for files under 20 lines.

5. **Macro-heavy Rust code**: `#[derive()]`, `macro_rules!`, and proc macros
   may inflate LOC counts or create false god-object signals. Note in findings
   when macros are the likely cause.

6. **Multi-language repositories**: Apply language-specific checks only to
   matching file extensions. See reference.md for the full language mapping.

7. **Comment-heavy files**: Comments and blank lines inflate LOC counts. Use
   logical LOC (non-blank, non-comment lines) when feasible; fall back to
   raw LOC with a note.

8. **Trait implementations in Rust**: A struct implementing multiple traits may
   look like a god object by method count but have clear single responsibility.
   Check whether methods come from trait impls before flagging.
