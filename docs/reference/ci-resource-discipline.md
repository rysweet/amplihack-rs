# CI Resource Discipline Reference

> [Home](../index.md) > Reference > CI Resource Discipline

> [PLANNED - Implementation Pending]
>
> This page defines the CI resource contract to apply while implementing the
> provider-neutral workflow feature. Existing workflows should be updated to match
> this reference when PR 6 lands.

CI workflows use explicit concurrency, timeout, matrix, and cache boundaries so
required coverage remains reliable without wasting runner resources.

## Concurrency

Every workflow defines a concurrency group scoped to the workflow, ref, and pull
request when available:

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: true
```

Release workflows that publish immutable artifacts use `cancel-in-progress:
false` for publish jobs and keep validation jobs cancellable.

## Timeouts

Jobs set explicit `timeout-minutes`. Long-running jobs split expensive setup,
build, and test phases so failures report the constrained phase rather than
timing out the whole workflow.

| Job type | Typical timeout |
| --- | --- |
| Formatting and docs checks | 10 minutes |
| Rust unit tests without heavy native backends | 30 minutes |
| Workspace tests with native backends | 60 minutes |
| Release packaging | 45 minutes per target |

## Matrix boundaries

Matrices cover meaningful compatibility dimensions only:

- operating system support promised by the release
- Rust stable toolchain used by the project
- feature flags that change behavior materially
- provider simulation scenarios that exercise distinct contracts

Provider behavior is tested with deterministic simulation instead of live
GitHub/Azure DevOps matrix jobs.

## Cache boundaries

Caches are keyed by dependency lockfiles, toolchain, operating system, and target
where relevant. Cache restore is an optimization only; CI must pass from a cold
cache.

Rust build caches exclude workflow runtime output, provider fixture output, logs,
and generated simulation artifacts.

## Required coverage

CI keeps required coverage while reducing avoidable resource use:

| Surface | Required validation |
| --- | --- |
| Provider domain logic | Rust unit tests in `amplihack-workflows`. |
| CLI provider adapters | Adapter tests with fake command runners. |
| Recipe behavior | Deterministic recipe simulations. |
| Runtime isolation | Tests or simulation proving runtime output stays outside commit worktrees. |
| Terminal state | Unit tests plus representative recipe simulation paths. |
| Documentation | Link and markdown checks used by the docs workflow. |

## Repository workflow mapping

Apply the resource contract to the existing workflow files:

| Workflow file | Jobs | Required resource discipline |
| --- | --- | --- |
| `.github/workflows/ci.yml` | `check`, `test`, `install-smoke`, `cross-compile`, `release` | Add workflow/ref concurrency, job timeouts, cache keys scoped by target, and keep `test` using `cache-targets: false` for native-heavy workspace tests. |
| `.github/workflows/docs.yml` | `build`, `deploy` | Keep docs-only path filters, add a 10-minute build timeout, and keep deploy dependent on strict build success. |
| `.github/workflows/release.yml` | `version-bump`, `build`, `release` | Use non-cancellable publish sections, keep matrix bounded to release targets, and set per-job release timeouts. |
| `.github/workflows/publish-snapshot.yml` | `cross-compile`, `release` | Keep Rust-only path filters, bound the target matrix to supported snapshot targets, and set packaging timeouts. |
| `.github/workflows/atlas.yml` | `build-atlas` | Keep artifact retention explicit and add a docs/atlas scoped path filter when atlas generation becomes expensive. |
| `.github/workflows/invisible-char-scan.yml` | `scan` | Keep safe base checkout, add timeout, and keep permissions read-only. |

Provider-neutral workflow tests belong in `.github/workflows/ci.yml` as unit
tests and deterministic simulations. Do not add live GitHub/Azure DevOps matrix
jobs for provider behavior.

## Implementation checklist

1. Add or verify workflow-level concurrency for every workflow.
2. Add `timeout-minutes` to every job.
3. Keep Rust cache restore optional and keyed by lockfile, target, and runner.
4. Keep provider simulations deterministic and offline.
5. Ensure generated simulation and runtime artifacts are not cached or uploaded
   unless explicitly needed for debugging.
6. Keep release publishing non-cancellable once artifact publication starts.

## See also

- [Recipe Simulation Reference](workflow-simulation.md)
- [Workflow Runtime Artifacts Reference](workflow-runtime-artifacts.md)
- [Provider-Neutral Workflow API](workflow-provider-contract.md)
