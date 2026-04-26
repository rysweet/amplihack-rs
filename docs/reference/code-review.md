# Code Review Practices

**Type**: Reference (Information-Oriented)

Code review practices and philosophy compliance checks for amplihack
contributions.

## Review Checklist

### Philosophy Compliance

| Principle              | Check                                              |
| ---------------------- | -------------------------------------------------- |
| Ruthless Simplicity    | No unnecessary abstractions or duplicated code     |
| Zero-BS                | No stubs, TODOs, or placeholder implementations    |
| Error Visibility       | All errors logged with context, no silent failures |
| Regeneratable Modules  | No hardcoded paths or assumptions                  |
| Present-Moment Focus   | Solves actual problems, not hypothetical ones      |

### Code Quality

- [ ] No duplicated code (extract helpers if needed)
- [ ] Proper error handling with context
- [ ] Type-safe (passes `cargo clippy -- -D warnings`)
- [ ] Properly formatted (`cargo fmt --all`)
- [ ] Tests cover new functionality
- [ ] No hardcoded credentials or paths
- [ ] Subprocess calls use arrays, not shell strings

### Security

- [ ] No hardcoded credentials
- [ ] No injection vulnerabilities
- [ ] Proper timeout handling
- [ ] Secure error messages (no sensitive data leakage)
- [ ] Subprocess inputs validated

## Common Issues and Fixes

### Code Duplication

**Before** (3 copies of the same logic):

```rust
// In function A
let result = process_input(input);
if result.is_err() { log_error(&result); }

// In function B — identical
let result = process_input(input);
if result.is_err() { log_error(&result); }
```

**After** (extracted helper):

```rust
fn process_and_log(input: &Input) -> Result<Output> {
    let result = process_input(input)?;
    Ok(result)
}
```

### Missing Error Handling

**Before** (silent failure):

```rust
let _ = subprocess.run(args);
```

**After** (visible error):

```rust
match subprocess.run(args) {
    Ok(output) => output,
    Err(e) => {
        tracing::warn!("Subprocess failed: {e}");
        return Err(e.into());
    }
}
```

### Incomplete Implementation

**Before** (stub):

```rust
fn generate_summary() -> String {
    todo!("implement later")
}
```

**After** (complete or removed):

```rust
fn generate_summary(results: &[Result]) -> String {
    format!("Completed {} tasks: {}", results.len(),
        results.iter().filter(|r| r.is_ok()).count())
}
```

## Review Workflow

Code review happens at step 11 of the
[Default Workflow](../concepts/default-workflow.md):

1. **Self-review** — author checks against this checklist
2. **Agent review** — reviewer agent checks philosophy compliance
3. **CI validation** — automated lint, test, build checks
4. **Merge** — after all checks pass

## Related

- [Philosophy](../concepts/philosophy.md) — design principles
- [Developing amplihack](../howto/develop-amplihack.md) — development setup
- [Default Workflow](../concepts/default-workflow.md) — the 23-step workflow
