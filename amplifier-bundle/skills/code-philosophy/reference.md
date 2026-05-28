# Code Philosophy Audit — Reference Guide

Detailed detection criteria, code examples, severity escalation rules, and
report format specification for the code-philosophy audit skill.

## Recipe Architecture

The code-philosophy skill is executed via the `code-philosophy-audit` recipe
(`amplifier-bundle/recipes/code-philosophy-audit.yaml`). The recipe orchestrates
five layers, each as a separate recipe step.

### Layer Interactions

```
Layer 1 (code-smell-detector)
  │ findings passed forward
  ▼
Layer 2 (philosophy-compliance-workflow)
  │ receives L1 findings for dedup, adds architecture findings
  ▼
Layer 3 (3-pass audit)
  │ receives L1+L2 findings for dedup, runs brick/quality/spirit passes
  ▼
Layer 4 (consolidation)
  │ merges all findings, deduplicates, sorts by severity
  ▼
Layer 5 (re-assessment) ← conditional: only if fixes were applied
```

Each layer receives the prior layers' findings as context and is responsible
for de-duplicating against them. Layer 4 does a final cross-layer merge.

### Finding Deduplication Logic

Findings are deduplicated by matching on `file + line (±3 lines) + category`.
When overlapping findings are detected across layers:

| Layer 1 Category | Overlapping Layer 3 Category | Resolution |
|-----------------|------------------------------|------------|
| over-abstraction | Pass 3: over-abstraction | Keep higher severity, merge descriptions |
| large-function | Pass 1: function-loc | Keep higher severity, note both sources |
| inheritance | Pass 1: inheritance | Keep higher severity, note both sources |

| Layer 2 Category | Overlapping Layer 3 Category | Resolution |
|-----------------|------------------------------|------------|
| ruthless-simplicity | Pass 3: ruthless-simplicity | Keep higher severity, merge descriptions |
| brick-modularity | Pass 3: brick-modularity | Keep higher severity, note both sources |

Non-overlapping findings pass through to consolidation unchanged.

### Running Individual Layers vs Full Audit

| Mode | Command |
|------|---------|
| Full audit (all 5 layers) | `amplihack recipe run code-philosophy-audit -c target_path="src/" -c task_description="Full audit" -c repo_path=.` |
| Layer 1 only | `Skill(skill="code-smell-detector")` |
| Layer 2 only | `Skill(skill="philosophy-compliance-workflow")` |
| Layer 3 only | `Skill(skill="code-philosophy")` — launches full recipe if runtime supports `recipe:` frontmatter |

Audit scope is controlled by `target_path`: a file, directory, or empty for full repo.

## Configuration Reference

All context variables passed via `-c key=value` to the recipe:

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `repo_path` | path | `"."` | Repository root. All file paths are relative to this. |
| `target_path` | path | `""` | Audit scope. A file, directory, or empty for full repo. |
| `task_description` | string | `""` | Human-readable audit context. For diff/PR audits, include the diff command (e.g., `"Audit PR #42: gh pr diff 42"`). |

**Computed variables** (set by recipe steps, not user-configurable):

| Variable | Set By | Description |
|----------|--------|-------------|
| `layer_1_findings` | Layer 1 | JSON findings from code-smell-detector |
| `layer_2_findings` | Layer 2 | JSON findings from philosophy-compliance-workflow |
| `layer_3_findings` | Layer 3 | JSON findings from 3-pass audit |
| `consolidation_report` | Layer 4 | Unified deduplicated report with verdict |
| `fix_results` | External | Set externally by user/dev-orchestrator after fixes are applied. The recipe does NOT invoke dev-orchestrator itself — fix delegation is a manual step between Layer 4 output and Layer 5 re-assessment. |
| `reassessment_report` | Layer 5 | Post-fix re-assessment results |

**Recursion limits** (set in recipe header, not overridable):

| Setting | Value | Purpose |
|---------|-------|---------|
| `max_depth` | 4 | Maximum nesting depth for agent sub-calls |
| `max_total_steps` | 20 | Hard cap on total recipe steps to prevent runaway |

## Integration Guide

### With dev-orchestrator

The audit produces findings but does not modify code. To complete the
fix→verify cycle:

1. Run the audit: `amplihack recipe run code-philosophy-audit -c ...`
2. Review the consolidated report (Layer 4 output)
3. For FAIL/PASS-WITH-WARNINGS verdicts, pass findings to dev-orchestrator:
   ```
   Skill(skill="dev-orchestrator")
   Task: "Fix philosophy violations from audit report: <paste findings>"
   ```
4. After fixes, re-run the audit. If `fix_results` context is populated,
   Layer 5 runs automatically on changed files only.

### In CI/CD Pipelines

The recipe runs as a standard `amplihack recipe run` command. Integrate
into CI by checking the exit code and parsing the JSON verdict:

```yaml
# GitHub Actions example
- name: Philosophy Audit
  run: |
    amplihack recipe run code-philosophy-audit \
      -c target_path="src/" \
      -c task_description="CI audit on push" \
      -c repo_path=. 2>&1 | tee audit-output.log
    # Parse verdict from output
    if grep -q '"verdict": "FAIL"' audit-output.log; then
      echo "::error::Philosophy audit FAIL — see findings"
      exit 1
    fi
```

### With quality-audit-cycle

The `quality-audit-cycle` recipe can invoke `code-philosophy-audit` as a
sub-recipe for periodic quality gates. The two recipes are complementary:
- `code-philosophy-audit` — single-run audit with fix delegation
- `quality-audit-cycle` — recurring audit loop with trend tracking

### PR Review Integration

For pull request reviews, combine with the skill trigger:

```
Skill(skill="code-philosophy")
Target: PR #42
```

The skill parses the PR number, invokes the recipe with
`task_description="Audit PR #42: gh pr diff 42"`, and returns
the consolidated report as a review comment.

## Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| "agent not found" | `amplihack:core:reviewer` not registered | Run `amplihack install` to stage bundled agents |
| Layer 5 never runs | `fix_results` not set externally | Set `fix_results` after dev-orchestrator applies fixes; skipped by design in standalone audits |
| Duplicate findings | Different categories or line numbers >3 apart | Check `dedup_stats` in Layer 4 output |
| Audit too slow | Full repo scan on large codebase | Use `target_path` to scope; for >10K files, audit per-directory |
| False positives on generated code | Missing codegen markers | Add `// Code generated` or `@generated` to file header; Phase 0 auto-detects and demotes to **low** |

## Performance Optimization Guide

### Phase 0 Caching

Phase 0 (file classification) is the single most impactful optimization
point. Its output — file paths, LOC counts, language tags, exclusion flags —
is reused by all three passes. Rules:

1. **Enumerate once**: A single `find` + `wc -l` + `head` pipeline collects
   all data. No pass may re-run `find` or `wc -l`.
2. **Diff-mode shortcut**: For diff-based audits, replace the directory walk
   with `git diff --name-only` (staged) or `gh pr diff --name-only` (PR).
   This reduces the file list from thousands to tens.
3. **Store as a lookup table**: Build a dict/map keyed by file path so
   passes can look up LOC, language, and exclusions in O(1).

### Combined Regex Scanning

Instead of running N separate greps per check type, run **one combined grep
per language bucket**:

```bash
# Rust: 1 grep covers unwrap + panic + unsafe + todo + let-ignore
grep -nE '\.unwrap\(\)|panic!\(|unsafe \{|unsafe fn|todo!\(\)|unimplemented!\(\)|let _ =' src/**/*.rs

# Python: 1 grep covers swallowed exceptions + stubs + naming
grep -nE 'except.*:.*pass|raise NotImplementedError|# TODO|# FIXME|class .*(Manager|Helper|Util)' **/*.py
```

Then categorize each match line by pattern. This turns 6+ tool calls per
language into 1.

### Parallel Execution

Within Layer 3, **Pass 2 and Pass 3 are independent** — both depend on
Phase 0 and Pass 1 but not on each other. Agent runtimes supporting
parallel tool calls should execute them concurrently. Expected speedup:
~40% reduction in Layer 3 wall-clock time on average.

At the recipe level, Layers 1→2→3 must run sequentially (each receives
prior findings for dedup). There is no safe parallelism across layers.

### Cross-Layer Dedup Efficiency

Layers 2, 3, and 4 all deduplicate against prior findings. The efficient
approach:

1. **Before scanning**, extract `file:line:category` tuples from prior
   layer findings into a skip-set (hash set or dict)
2. **During scanning**, check each candidate location against the skip-set
   — O(1) lookup per candidate
3. **Do NOT** re-parse the full JSON of prior findings for each check

This avoids O(N×M) comparison where N = current findings and M = prior
findings.

### Layer 5 Scoped Re-checks

Layer 5 (re-assessment) should not blindly re-run all checks on changed
files. Instead:

1. Map each changed file to the original finding categories on that file
2. Re-run only those check categories (e.g., if only `unwrap` was flagged,
   skip brick-rule and philosophy-spirit checks)
3. Run a lightweight scan for NEW violation types not in the original report
4. Expected savings: 50-70% fewer checks when fixes target specific issues

### Token Budget Management

The recipe passes `{{layer_N_findings}}` as raw text into subsequent layer
prompts. For large codebases, this can consume significant tokens. Strategies:

- **Agents should output concise findings**: Use short evidence snippets
  (1-2 lines), not full function bodies
- **Layer 4 consolidation**: If combined findings exceed token limits,
  prioritize critical > high > medium > low and truncate low findings
- **Layer 5 receives only the consolidated report**, not all three layers'
  raw output — this is already optimized by the recipe design

### Early Exit Conditions

| Condition | Optimization |
|-----------|-------------|
| Phase 0 finds 0 files | Skip all passes, emit PASS |
| Pass 1 flags file for rewrite (>3 critical) | Skip Pass 2/3 on that file |
| All three layers report 0 findings | Layer 4 short-circuits to PASS |
| Layer 5 changed-file list is empty | Skip re-assessment |

---

## Pass 1: BRICK RULE Compliance — Detection Criteria

### 1.1 File LOC Limit (≤400 lines per file)

**Detection**: Count total lines of code per file. Files exceeding 400 LOC
violate the brick philosophy's self-contained module principle.

**Severity**: **critical** — a file over 400 LOC is a critical violation, too large to be a
regeneratable brick.

**How to count**: Use `wc -l` or equivalent. When feasible, count logical LOC
(non-blank, non-comment lines) for a more accurate measure.

❌ **BAD** — monolithic 600-line file:
```rust
// src/engine.rs — 600+ lines covering parsing, validation, execution, and reporting
pub struct Engine { /* 15 fields */ }
impl Engine {
    pub fn parse(&self) { /* 80 lines */ }
    pub fn validate(&self) { /* 120 lines */ }
    pub fn execute(&self) { /* 200 lines */ }
    pub fn report(&self) { /* 150 lines */ }
}
```

✅ **GOOD** — split into focused modules:
```rust
// src/parser.rs — ~100 lines
pub struct Parser { /* 3 fields */ }
impl Parser { pub fn parse(&self) { /* 80 lines */ } }

// src/validator.rs — ~120 lines
// src/executor.rs — ~200 lines (still within 400 LOC)
// src/reporter.rs — ~150 lines
```

### 1.2 Function LOC Limit (≤50 lines per function)

**Detection**: Scan for function/method definitions and count body lines.
Functions exceeding 50 LOC should be decomposed.

**Severity**: **high** — a function over 50 lines is a high-severity violation indicating multiple responsibilities.

❌ **BAD** — 80-line function doing too much:
```python
def process_data(self, data):
    # validate (20 lines)
    ...
    # transform (30 lines)
    ...
    # persist (30 lines)
    ...
```

✅ **GOOD** — decomposed into focused functions:
```python
def process_data(self, data):
    validated = self._validate(data)
    transformed = self._transform(validated)
    self._persist(transformed)
```

### 1.3 God Object Detection

**Detection**: A struct/class with >10 fields OR >10 methods signals multiple
responsibilities. Check for god objects that try to do everything.

**Severity**: **high** — god objects violate single-responsibility.

❌ **BAD** — god object with multiple responsibilities:
```rust
pub struct AppState {
    db: Database,
    cache: Cache,
    logger: Logger,
    mailer: Mailer,
    queue: Queue,
    config: Config,
    metrics: Metrics,
    auth: AuthService,
    storage: Storage,
    scheduler: Scheduler,
    notifier: Notifier,  // 11 fields — god object
}
```

✅ **GOOD** — separated concerns:
```rust
pub struct RequestContext {
    db: Database,
    cache: Cache,
    config: Config,
}
```

### 1.4 Deep Inheritance Detection

**Detection**: Trace class/trait inheritance chains. Depth >2 levels violates
the preference for composition over deep inheritance.

**Severity**: **medium** — deep inheritance makes code harder to understand
and regenerate.

❌ **BAD** — inheritance depth >2 levels:
```python
class Base: ...
class Middle(Base): ...
class Child(Middle): ...
class GrandChild(Child): ...  # depth = 3, violation
```

✅ **GOOD** — flat composition:
```python
class Handler:
    def __init__(self, validator, executor):
        self.validator = validator
        self.executor = executor
```

## Pass 2: QUALITY INVARIANTS — Detection Criteria

### 2.1 unwrap/panic in Production Code (Rust/.rs only)

**Detection**: Search for `.unwrap()` and `panic!()` in `.rs` files that are
NOT test files. Test files (paths containing `/tests/`, `_test.rs`, `test_`,
`tests.rs`) are exempt.

**Severity**: **high** — unwrap/panic can crash production systems.

**Pattern**: `grep -n '\.unwrap()' --include='*.rs'` then exclude test paths.

❌ **BAD** — unwrap in production code:
```rust
// src/config.rs
let port = env::var("PORT").unwrap();  // crash if PORT unset
```

✅ **GOOD** — proper error handling with Result:
```rust
// src/config.rs
let port = env::var("PORT")
    .map_err(|_| ConfigError::MissingEnv("PORT"))?;
```

### 2.2 unsafe Code Blocks (Rust/.rs only)

**Detection**: Search for `unsafe {` or `unsafe fn` in `.rs` files.

**Severity**: **medium** — unsafe code requires justification. Not always wrong,
but must be documented and minimized.

### 2.3 Error Handling — No Swallowed Exceptions

**Detection**: Look for empty catch/except blocks, ignored Result values, and
missing error propagation across all languages.

**Severity**: **high** — swallowed errors hide bugs.

| Language | Anti-Pattern | Detection |
|----------|-------------|-----------|
| Rust | `let _ = fallible_fn()` | Ignored `Result` binding |
| Python | `except: pass` or `except Exception: pass` | Empty except blocks |
| JavaScript | `catch(e) {}` | Empty catch blocks |
| Go | `_, _ = fn()` without error check | Discarded error return |
| Shell/Bash | Missing `set -e` or unchecked `$?` | No error propagation |

❌ **BAD** — swallowed exception:
```python
try:
    save_data(record)
except Exception:
    pass  # silently ignores all errors
```

✅ **GOOD** — error handled transparently:
```python
try:
    save_data(record)
except DatabaseError as e:
    logger.error(f"Failed to save record: {e}")
    raise
```

### 2.4 Test-to-Production Ratio

**Detection**: Calculate the ratio of test LOC to implementation LOC for
each module or changed file set.

**Severity**: **medium** — imbalanced ratios indicate either under-testing
or over-testing.

Target ratios from PHILOSOPHY.md (Proportional engineering):

| Change Type | Target Ratio | Red Flag |
|-------------|-------------|----------|
| Config changes | 1:1 to 2:1 | >5:1 |
| Simple functions | 2:1 to 4:1 | >10:1 |
| Business logic | 3:1 to 8:1 | >15:1 |
| Critical paths | 5:1 to 15:1 | >20:1 |

A ratio >20:1 is always a red flag indicating likely over-testing.
A ratio <1:1 for business logic indicates insufficient testing.

### 2.5 Install-Completeness Invariant

**Detection**: When new components (binaries, assets, hooks) are added, verify
that BOTH the install staging code AND the post-install verifier are updated
in the same change. A new component missing its verifier entry violates the
install-completeness invariant.

**Severity**: **critical** — install must fail loudly if a component cannot be
staged. Silent success with missing components breaks user trust.

### 2.6 Stubs, TODOs, and Dead Code

**Detection**: Search for placeholder markers that indicate incomplete
implementation:

| Pattern | Language | Detection |
|---------|----------|-----------|
| `todo!()` | Rust | `grep 'todo!()'` |
| `unimplemented!()` | Rust | `grep 'unimplemented!()'` |
| `TODO` | All | `grep -i 'TODO'` in non-comment contexts |
| `FIXME` | All | `grep -i 'FIXME'` |
| `stub` | All | Functions with empty bodies or pass-only |
| `placeholder` | All | Placeholder values or functions |

**Severity**: **medium** — stubs and TODOs violate zero-BS implementation.

## Pass 3: PHILOSOPHY SPIRIT — Detection Criteria

### 3.1 Ruthless Simplicity

**Detection**: Look for over-engineered solutions where simpler alternatives
exist. Indicators include unnecessary design patterns, gratuitous generics,
and solutions more complex than the problem.

**Severity**: **medium**.

**Signals**:
- Factory patterns for objects created in one place
- Strategy patterns with a single strategy
- Observer patterns with a single observer
- Builder patterns for structs with 2-3 fields

### 3.2 Zero-BS Naming

**Detection**: Flag vague, non-descriptive names that hide what code actually
does. These naming anti-patterns signal unclear thinking about responsibility:

| Anti-Pattern Name | Why It's Bad |
|-------------------|-------------|
| `Manager` | What does it manage? Usually a god object in disguise |
| `Helper` | Helpers have no defined responsibility |
| `Util` / `Utils` | Junk drawer for unrelated functions |
| `Handler` | Too generic — handle what, exactly? |
| `Processor` | Process how? Too vague to be useful |
| `Base` class | Signals unnecessary inheritance hierarchy |
| `Service` (alone) | Meaningless without a domain qualifier |

**Severity**: **medium** — bad names indicate unclear design.

✅ **GOOD** naming:
```rust
// Instead of "DataProcessor" → "CsvParser"
// Instead of "RequestHandler" → "AuthMiddleware"
// Instead of "BaseManager" → just delete it
```

### 3.3 Brick Modularity

**Detection**: Verify modules are self-contained bricks with clear boundaries.
Each module should be regeneratable from its specification without breaking
connections. Check for:

- Circular dependencies between modules
- Modules reaching into other modules' internals
- Missing module boundary definitions (no public API)
- Non-isolated test fixtures (tests depending on other module state)

**Severity**: **medium**.

### 3.4 Over-Abstraction

**Detection**: Identify unnecessary abstraction layers that add complexity
without proportional value:

- Single-implementation traits/interfaces (not needed for testing)
- Wrapper types that add no behavior or safety
- More than 3 abstraction layers for a single feature
- Unnecessary indirection (calling through 3+ layers to reach actual logic)

**Severity**: **high** — over-abstraction directly violates ruthless simplicity.

❌ **BAD** — unnecessary abstraction layer:
```rust
trait DataStore { fn save(&self, data: &Data) -> Result<()>; }
struct PostgresStore { /* single impl */ }
impl DataStore for PostgresStore { /* only implementation */ }
// Trait adds no value — no second impl, no test mock needed
```

✅ **GOOD** — direct implementation:
```rust
struct PostgresStore { /* fields */ }
impl PostgresStore {
    pub fn save(&self, data: &Data) -> Result<()> { /* impl */ }
}
```

### 3.5 Sycophancy in Comments

**Detection**: Flag praise words and flattery in code comments that add no
technical value. These are sycophantic patterns that violate the project's
anti-sycophancy stance (see TRUST.md):

**Words to detect**: Great, Excellent, Amazing, Beautiful, Brilliant, Wonderful,
Perfect, Awesome, Fantastic, Superb

**Severity**: **low** — cosmetic but signals AI-generated sycophancy.

❌ **BAD** — sycophantic comment:
```python
# This is an Excellent implementation of the parser
# Beautiful error handling below
```

✅ **GOOD** — factual comment (or no comment at all):
```python
# Parses CSV with RFC 4180 quoting rules
```

### 3.6 Future-Proofing Anti-Patterns

**Detection**: Code built for hypothetical future requirements rather than
current needs. Signals include:

- "just in case" parameters or config options
- "maybe someday" comments justifying unused code
- Generic frameworks for one-time operations
- Elaborate plugin systems with no plugins
- Future-proof abstractions with no current second use

**Severity**: **medium** — violates "present-moment focus" from philosophy.

## Severity Escalation Rules

Severity may escalate or promote based on context:

| Condition | Escalation |
|-----------|-----------|
| Violation in a critical path (auth, payment, data persistence) | Bump one level (medium→high, high→critical) |
| Same violation repeated >5 times in one file | Bump one level |
| Violation in public API surface | Bump one level |
| Violation in generated or vendored code | Demote to **low** |
| Violation in test files | Demote to **low** or skip |
| New violation introduced in a diff (not pre-existing) | Bump one level when auditing diffs |

**Escalation threshold**: If a single file has >3 critical findings, flag the
entire file for rewrite consideration.

## Report Format Specification

### Report Header

```markdown
# Code Philosophy Audit Report

**Target**: <file/directory/diff description>
**Mode**: file | directory | git-diff | pr-diff
**Verdict**: PASS | FAIL | PASS-WITH-WARNINGS
**Date**: <ISO 8601 timestamp>
```

### Findings Table (per-pass breakdown)

```markdown
## Pass 1: BRICK RULE Findings

| # | Location (file:line) | Severity | Finding | Suggested Fix |
|---|---------------------|----------|---------|---------------|
| 1 | src/engine.rs:1 | critical | File exceeds 400 LOC (523 lines) | Split into parser, executor, reporter modules |

**Pass 1 Total**: 1 finding (1 critical, 0 high, 0 medium, 0 low)

## Pass 2: QUALITY INVARIANTS Findings
...

## Pass 3: PHILOSOPHY SPIRIT Findings
...
```

### Summary Section

```markdown
## Summary

| Pass Name | Critical | High | Medium | Low | Total |
|-----------|----------|------|--------|-----|-------|
| BRICK RULE | 1 | 0 | 0 | 0 | 1 |
| QUALITY INVARIANTS | 0 | 2 | 1 | 0 | 3 |
| PHILOSOPHY SPIRIT | 0 | 0 | 2 | 1 | 3 |
| **Total** | **1** | **2** | **3** | **1** | **7** |

**Verdict**: FAIL (1 critical finding)
- FAIL: Any critical finding present
- PASS-WITH-WARNINGS: No critical, but high or medium findings present
- PASS: No critical, high, or medium findings (low-only or clean)
```

### Per-Pass Verdict Rules

Each pass produces its own count and verdict. The overall verdict is the
worst verdict across all passes.

## Language-Specific Detection Patterns

### Combined Detection Commands

To minimize tool calls, run a single combined grep per language bucket from
Phase 0. These patterns cover Pass 2 and Pass 3 checks simultaneously.

**Rust** (production files only — exclude test-tagged files):
```bash
grep -nE '\.unwrap\(\)|panic!\(|unsafe \{|unsafe fn|todo!\(\)|unimplemented!\(\)|let _ =' src/**/*.rs
```

**Python**:
```bash
grep -nE 'except.*:.*pass|raise NotImplementedError|# TODO|# FIXME|class .*(Manager|Helper|Util|Handler|Processor|Base)' **/*.py
```

**JavaScript/TypeScript**:
```bash
grep -nE 'catch\s*\([^)]*\)\s*\{\s*\}|\.catch\(\s*\(\)\s*=>\s*\{\s*\}\)|// TODO|// FIXME|throw new Error\(.not implemented' **/*.{js,ts,tsx}
```

**Go**:
```bash
grep -nE '_, _ =|// TODO|// FIXME|panic\("not implemented' **/*.go
```

**Shell**:
```bash
grep -nEL 'set -e' **/*.sh   # files MISSING error handling
```

After the combined scan, categorize each match into its specific check type
for the findings table. This turns N separate scans into 1 per language.

### Rust (.rs files)

| Check | Pattern | Notes |
|-------|---------|-------|
| unwrap | `.unwrap()` | Skip in test files |
| panic | `panic!()` | Skip in test files |
| unsafe | `unsafe {`, `unsafe fn` | Always flag, may be justified |
| Error handling | `let _ =` on Result types | Swallowed errors |
| God object | `struct` field count, `impl` method count | Check trait impls separately |
| Stubs | `todo!()`, `unimplemented!()` | Zero-BS violation |

### Python (.py files)

| Check | Pattern | Notes |
|-------|---------|-------|
| Swallowed exceptions | `except: pass`, `except Exception: pass` | Must handle or re-raise |
| Stubs | `pass` as sole function body, `raise NotImplementedError` | Zero-BS violation |
| God object | Class with >10 methods or >10 attributes in `__init__` | Multiple responsibilities |
| Naming | `Manager`, `Helper`, `Util` class names | Vague responsibility |
| Inheritance | Multiple levels of class inheritance | Prefer composition |

### JavaScript/TypeScript (.js, .ts, .tsx files)

| Check | Pattern | Notes |
|-------|---------|-------|
| Swallowed errors | `catch(e) {}`, `.catch(() => {})` | Empty catch blocks |
| Stubs | `// TODO`, `throw new Error('not implemented')` | Zero-BS violation |
| God object | Class/object with >10 methods | Multiple responsibilities |
| Over-abstraction | Abstract class with single subclass | Unnecessary layer |

### Go (.go files)

| Check | Pattern | Notes |
|-------|---------|-------|
| Ignored errors | `_, _ = fn()` | Must check error returns |
| God object | Struct with >10 fields | Multiple responsibilities |
| Stubs | `// TODO`, `panic("not implemented")` | Zero-BS violation |

### Shell/Bash (.sh files)

| Check | Pattern | Notes |
|-------|---------|-------|
| Error handling | Missing `set -euo pipefail` | Must fail on errors |
| Stubs | `# TODO`, empty function bodies | Zero-BS violation |

## Proportionality Principle

From PHILOSOPHY.md — effort must be proportional to complexity and criticality.
When evaluating test-to-prod ratios, use the target ratio ranges. Do not flag
a 4:1 ratio on business logic as a problem — that is within the proportional
range. Only flag ratios that exceed the red flag thresholds.

When reporting, prioritize critical and high findings. Do not overwhelm the
report with low-severity cosmetic issues. If a file has 20+ low-severity
findings but zero critical/high, the verdict is still PASS-WITH-WARNINGS
at most.

## Output Schema Reference

Each layer produces structured JSON output. The schemas below define the
contract between layers and with downstream consumers.

### Layer 1/2 Finding Schema

```json
{
  "layer": 1,
  "source": "code-smell-detector",
  "findings": [{
    "id": "L1-001",
    "category": "over-abstraction|inheritance|large-function|tight-coupling|missing-exports",
    "severity": "critical|high|medium|low",
    "file": "path/to/file.rs",
    "line": 42,
    "description": "Single-implementation trait with no test mock usage",
    "evidence": "trait DataStore used only by PostgresStore",
    "fix": "Remove trait, use PostgresStore directly"
  }],
  "summary": {"total": 1, "critical": 0, "high": 1, "medium": 0, "low": 0}
}
```

### Layer 3 Finding Schema

Layer 3 adds `pass` and `classification` fields:

```json
{
  "layer": 3,
  "source": "code-philosophy-3-pass",
  "classification": {
    "total_files": 47,
    "by_language": {"rust": 32, "python": 10, "shell": 5},
    "excluded": ["vendor/lib.rs (vendored)", "build.rs (generated)"]
  },
  "findings": [{
    "id": "L3-P1-001",
    "pass": "brick-rule",
    "category": "loc-limit",
    "severity": "critical",
    "file": "src/engine.rs",
    "line": 1,
    "description": "File exceeds 400 LOC (523 lines)",
    "evidence": "wc -l: 523",
    "fix": "Split into parser.rs, executor.rs, reporter.rs"
  }],
  "summary": {
    "pass_1": {"total": 1, "critical": 1, "high": 0, "medium": 0, "low": 0},
    "pass_2": {"total": 0, "critical": 0, "high": 0, "medium": 0, "low": 0},
    "pass_3": {"total": 0, "critical": 0, "high": 0, "medium": 0, "low": 0},
    "overall": {"total": 1, "critical": 1, "high": 0, "medium": 0, "low": 0}
  }
}
```

### Layer 4 Consolidated Report Schema

```json
{
  "layer": 4,
  "source": "consolidation",
  "verdict": "FAIL",
  "dedup_stats": {
    "total_raw": 12,
    "duplicates_removed": 3,
    "total_consolidated": 9
  },
  "findings": [{
    "id": "C-001",
    "source_layers": [1, 3],
    "category": "over-abstraction",
    "severity": "high",
    "file": "src/store.rs",
    "line": 15,
    "description": "Single-impl trait (L1) + over-abstraction (L3 Pass 3)",
    "evidence": "trait DataStore → only PostgresStore impl",
    "fix": "Remove trait, use concrete type"
  }],
  "summary": {
    "by_severity": {"critical": 1, "high": 3, "medium": 4, "low": 1},
    "by_source": {"layer_1": 4, "layer_2": 2, "layer_3": 3},
    "by_category": {"over-abstraction": 2, "loc-limit": 1, "unwrap": 3}
  },
  "verdict_reason": "1 critical finding: src/engine.rs exceeds 400 LOC"
}
```

### Layer 5 Re-Assessment Schema

```json
{
  "layer": 5,
  "source": "reassessment",
  "changed_files": ["src/engine.rs", "src/parser.rs", "src/executor.rs"],
  "original_findings_resolved": 7,
  "new_findings_introduced": 1,
  "verdict": "PASS-WITH-WARNINGS",
  "findings": [{
    "id": "R-001",
    "type": "new",
    "category": "naming",
    "severity": "medium",
    "file": "src/executor.rs",
    "line": 5,
    "description": "Class named 'ExecutorHelper' uses vague Helper suffix",
    "fix": "Rename to CommandRunner or TaskExecutor"
  }],
  "summary": {
    "original_total": 9,
    "resolved": 7,
    "remaining": 2,
    "new_violations": 1
  },
  "verdict_reason": "No critical findings; 1 new medium finding introduced"
}
```

### Verdict Decision Table

| Condition | Verdict |
|-----------|---------|
| Any `critical` finding present | `FAIL` |
| No `critical`, but `high` or `medium` present | `PASS-WITH-WARNINGS` |
| Only `low` findings or no findings | `PASS` |
