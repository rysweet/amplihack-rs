# Code Synthesis — Honest Errors Instead of Stub Output

**`CodeSynthesizer::generate` and `CodeSynthesizer::refactor` never fabricate
code. When no synthesis backend can honestly produce a result, they return an
explicit `Err`, not an `Ok` wrapping a `todo!()`, `NotImplementedError`, or
`// TODO` placeholder. Callers can no longer mistake a stub for real,
runnable code.**

> [Home](../index.md) > [Features](README.md) > Code Synthesis — Honest Errors

## Quick Navigation

- [Code Synthesizer API reference](../reference/domain-agents-api.md#code-synthesizer)
- [Error taxonomy](#error-taxonomy)
- [Usage](#usage)
- [Examples](#examples)

## What This Feature Does

`CodeSynthesizer` lives in the `amplihack-domain-agents` crate
(`crates/amplihack-domain-agents/src/code_synthesis.rs`). It exposes three
operations:

| Method       | Signature                                              | Behavior |
| ------------ | ------------------------------------------------------ | -------- |
| `generate`   | `fn generate(&self, spec: &CodeSpec) -> Result<GeneratedCode>` | Honest error — no synthesis backend is wired. |
| `refactor`   | `fn refactor(&self, code: &str) -> Result<GeneratedCode>`      | Honest error — no refactoring backend is wired. |
| `analyze`    | `fn analyze(&self, code: &str) -> Result<CodeAnalysis>`        | Real deterministic heuristic (unchanged). |

Before this feature, `generate` and `refactor` returned `Ok(GeneratedCode { .. })`
whose `code` field held a fabricated placeholder — a Rust `todo!(...)`, a Python
`raise NotImplementedError(...)`, or a `// TODO: Implement ...` comment. A silent
fallback is a silent failure: a caller receiving `Ok` reasonably trusts the
payload is usable code, then discovers at runtime (or worse, in production) that
it was never implemented. See issue
[#874](https://github.com/rysweet/amplihack-rs/issues/874).

This feature replaces every fabricated-`Ok` path with a typed `Err`, so a
missing capability is impossible to confuse with a real result.

## Error Taxonomy

The two failure modes are **distinct** and map to two different
[`DomainError`](../reference/domain-agents-api.md) variants. This preserves the
"distinguish *missing* from *corrupt*" contract: malformed input is reported
separately from a capability gap.

| Condition | Variant | Display text |
| --------- | ------- | ------------ |
| `generate`: `spec.description` or `spec.language` is empty/whitespace | `DomainError::InvalidInput` | `invalid input: code spec description and language must not be empty` |
| `generate`: well-formed spec, no backend available | `DomainError::CodeSynthesis` | `code synthesis error: code synthesis backend not available: cannot synthesize <language>` |
| `refactor`: `code` is empty/whitespace | `DomainError::InvalidInput` | `invalid input: code to refactor must not be empty` |
| `refactor`: non-empty `code`, no backend available | `DomainError::CodeSynthesis` | `code synthesis error: refactoring backend not available` |

- **`InvalidInput`** = the caller supplied nothing to work with (missing /
  corrupt input). Validation happens first, fail-closed, before any other logic.
- **`CodeSynthesis`** = the input was fine, but the capability to satisfy it is
  not available (capability gap).

In the `CodeSynthesis` message, `<language>` is the **trimmed** `spec.language`
(surrounding whitespace stripped, case preserved — e.g. `"  Rust  "` renders as
`cannot synthesize Rust`). It is the only caller-supplied value that appears in
any error: messages never interpolate `spec.description`, `spec.constraints`, or
the raw `code` body — see [Security](#security).

## Usage

```rust
use amplihack_domain_agents::{CodeSynthesizer, CodeSpec, DomainError};

let synth = CodeSynthesizer::with_defaults();

let spec = CodeSpec {
    description: "A function that adds two numbers".to_string(),
    language: "rust".to_string(),
    constraints: vec!["must be generic".to_string()],
};

match synth.generate(&spec) {
    Ok(generated) => {
        // Reserved for a future, real synthesis backend.
        println!("{}", generated.code);
    }
    Err(DomainError::CodeSynthesis(msg)) => {
        // Capability gap — reported honestly, never a stub.
        eprintln!("cannot synthesize yet: {msg}");
    }
    Err(DomainError::InvalidInput(msg)) => {
        // The spec was empty or whitespace-only.
        eprintln!("bad spec: {msg}");
    }
    Err(other) => eprintln!("unexpected: {other}"),
}
```

Because the method signatures are unchanged (`generate` and `refactor` already
returned `Result<GeneratedCode>`), callers that already handle `Result` compile
without modification. The behavioral change is `Ok(stub)` → `Err(honest)`.

## Examples

### `generate` — empty spec is `InvalidInput`

```rust
let synth = CodeSynthesizer::with_defaults();
let spec = CodeSpec {
    description: "   ".to_string(), // whitespace only
    language: "rust".to_string(),
    constraints: vec![],
};

let err = synth.generate(&spec).unwrap_err();
assert!(matches!(err, DomainError::InvalidInput(_)));
```

### `generate` — well-formed spec is `CodeSynthesis`

```rust
let synth = CodeSynthesizer::with_defaults();
let spec = CodeSpec {
    description: "A concurrent hash map with lock striping".to_string(),
    language: "rust".to_string(),
    constraints: vec!["thread-safe".to_string()],
};

let err = synth.generate(&spec).unwrap_err();
assert!(matches!(err, DomainError::CodeSynthesis(_)));
// The language token appears; the description/constraints never do.
assert!(err.to_string().contains("rust"));
assert!(!err.to_string().contains("concurrent hash map"));
```

### `generate` — the language token is trimmed

```rust
let synth = CodeSynthesizer::with_defaults();
let spec = CodeSpec {
    description: "A red-black tree".to_string(),
    language: "  Rust  ".to_string(), // surrounding whitespace
    constraints: vec![],
};

let err = synth.generate(&spec).unwrap_err();
// The language is trimmed (case preserved) before it is interpolated.
assert_eq!(
    err.to_string(),
    "code synthesis error: code synthesis backend not available: cannot synthesize Rust"
);
```

### `refactor` — empty vs. non-empty

```rust
let synth = CodeSynthesizer::with_defaults();

// Empty input → missing/corrupt → InvalidInput
assert!(matches!(
    synth.refactor("   ").unwrap_err(),
    DomainError::InvalidInput(_)
));

// Real code, but no backend → capability gap → CodeSynthesis
let err = synth
    .refactor("fn add(a: i32, b: i32) -> i32 { a + b }")
    .unwrap_err();
assert!(matches!(err, DomainError::CodeSynthesis(_)));
// The source body is never echoed back in the error message.
assert!(!err.to_string().contains("fn add"));
```

### `analyze` — unchanged, still returns `Ok`

`analyze` is a real, deterministic heuristic and is **out of scope** for this
change. It continues to return `Ok(CodeAnalysis { .. })`:

```rust
let synth = CodeSynthesizer::with_defaults();
let analysis = synth.analyze("fn hello() { println!(\"hi\"); }").unwrap();
assert!(analysis.complexity > 0);
```

## Configuration

No new configuration is introduced. `CodeSynthesizer` is still constructed the
same way, and `CodeSynthesisConfig` is unchanged:

```rust
use amplihack_domain_agents::{CodeSynthesizer, CodeSynthesisConfig};

// Explicit config
let synth = CodeSynthesizer::new(CodeSynthesisConfig {
    language: "python".to_string(),
    style: "pep8".to_string(),
    max_complexity: 5,
});

// Or defaults (language = "rust", style = "idiomatic", max_complexity = 10)
let synth = CodeSynthesizer::with_defaults();
```

The configured `language`/`style`/`max_complexity` continue to be stored and
returned by `config()`; they do not alter the honest-error contract above.

## Security

- **No information disclosure.** Error messages contain only static text plus,
  for `generate`, the trimmed `language` token. They never interpolate the spec
  description, the constraints list, or the raw `code` body. This is a net
  confidentiality improvement over the previous implementation, which echoed
  `spec.description` into the returned payload.
- **Validate first, fail closed.** Emptiness checks run before any other logic,
  so an empty/whitespace input can never reach a fabricated `Ok`.
- **No injection sink.** The `language` token is inserted with a positional
  `format!` argument, not as a format string, so a crafted `language` value
  cannot inject format directives. Inputs never reach a shell, SQL, filesystem,
  `eval`, or deserializer along these paths.
- **Total functions.** The synthesis paths contain no `unwrap`, `expect`,
  `panic!`, `todo!`, or `unreachable!` on any input. Every input maps to a
  well-defined `Ok` (for `analyze`) or `Err` (for `generate`/`refactor`).

## Scope Boundary

This feature changes **only** the failure semantics of `generate` and
`refactor` in `code_synthesis.rs`. It does **not**:

- add or wire a new LLM/synthesis backend,
- impose any wall-clock timeout (there is no long-running or network step here),
- derive control flow from string/marker/glyph parsing of model or tool output
  (no such output is parsed),
- modify `analyze`, `DomainError`, or the `CodeSpec` / `GeneratedCode` /
  `CodeAnalysis` models,
- change any method signature.

When a real synthesis backend is added later, it will populate the `Ok` arm of
the existing `Result<GeneratedCode>` contract — no caller change required.
