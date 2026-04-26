# Agent Bundle Generator

> **Status: Mixed.** The four-stage agent generator pipeline (analyze →
> plan → synthesize → assemble) ships today via the `amplihack new` CLI
> and the `amplihack-agent-generator` crate — see
> [Goal Agent Generator](./agent-generator.md). The **bundle / package /
> distribute** layer described on this page is currently a stub
> (`crates/amplihack-cli/src/cli_extensions.rs`) and the corresponding CLI
> surface is **planned, not shipped**.

This page describes how the generator output (a `GoalAgentBundle`) is
intended to be packaged and distributed. For the existing pipeline see the
[Goal Agent Generator](./agent-generator.md) concept doc and the
[Generate an Agent from a Goal](../howto/generate-agent-from-goal.md)
how-to.

## What "bundle" means here

A *bundle* is the on-disk output of `GoalAgentPackager` plus the metadata
required to publish, install, and update it as a unit:

```
my-agent/
├── manifest.json          # name, version, generator inputs
├── agent.md               # primary instructions
├── skills/                # synthesized skills
├── tools/                 # SDK tool wiring
└── tests/                 # generator-emitted smoke tests
```

The `manifest.json` shape is defined by `cli_extensions::generate_bundle`
today as a stub:

```json
{
  "name": "generated-agent",
  "version": "0.1.0",
  "prompt": "<original goal text>",
  "validated": false,
  "tested": false
}
```

Real validation, packaging, and distribution are tracked separately and
should not be assumed to work yet.

## Pipeline overlay (today vs. planned)

| Stage                     | Today                                 | Planned                                  |
|---------------------------|---------------------------------------|------------------------------------------|
| Analyze / Plan / Skills   | ✅ shipped (`amplihack new`)           | unchanged                                 |
| Assemble bundle on disk   | ✅ shipped (`GoalAgentPackager`)       | unchanged                                 |
| `generate_bundle()` API   | ⚠️ stub — writes empty manifest only   | full validation + smoke tests             |
| `package_bundle()` API    | ⚠️ stub — writes single placeholder    | tar.gz / zip with checksum                |
| `distribute_bundle()` API | ⚠️ stub                                | GitHub Releases, PyPI, local install      |
| `amplihack agent-generator bundle` CLI | ❌ not present in clap enum | proposed wrapper around the above        |

See `crates/amplihack-cli/src/cli_extensions.rs` for the current stub
shapes; replace them when filling in real implementations.

## Design constraints

When the bundle layer is implemented it must:

1. **Round-trip:** packaging then unpackaging must yield the exact bundle
   directory tree (no metadata drift).
2. **Be reproducible:** given the same generator inputs and pinned
   templates, the bundle bytes should be deterministic.
3. **Honor zero-BS:** validation / test failures must surface as non-zero
   exits, never silently degraded to "validated=false in manifest".
4. **Reuse existing primitives:** the agent generator pipeline already
   exists; the bundle layer is a thin wrapper, not a parallel pipeline.

## See also

- [Goal Agent Generator](./agent-generator.md)
- [Generate an Agent from a Goal](../howto/generate-agent-from-goal.md)
- [Use the Agent Bundle Generator](../howto/use-agent-bundle-generator.md)
- [Agent Generator API reference](../reference/agent-generator-api.md)
