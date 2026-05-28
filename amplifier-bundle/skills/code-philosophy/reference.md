# Code Philosophy Audit — Reference Guide

Detailed detection criteria, code examples, severity escalation rules, and
report format specification for the code-philosophy audit skill.

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
