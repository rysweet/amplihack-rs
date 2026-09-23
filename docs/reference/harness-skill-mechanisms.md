# Harness skill mechanisms

**What this is:** the discovery rules of every harness amplihack launches, written
down so nobody has to re-derive them from amplihack's own code. Claims are
sourced from the harness itself — its `--help`, its debug output, the literals
in its own binary, or a controlled experiment — never from what amplihack
currently does, which is the thing that kept being wrong. **Anything that was
reported rather than observed here says so explicitly, and anything that could
not be determined is marked undetermined.** A guess is what created this
situation; an honest gap is more useful than a confident invention.

**Why it exists.** The same bug shipped three times, each time as work that
silently produced nothing:

| Issue | What was published | What the harness actually reads | How it surfaced |
| --- | --- | --- | --- |
| #1344 | slash commands staged for Copilot only | `~/.claude/commands/<ns>/` | `/lock`, `/auto` etc. never existed in Claude |
| #1438 | skills staged to `~/.amplihack/.claude/skills` | `~/.claude/skills/` | new bundle skills invisible until some later interactive launch |
| #1449 | nothing — one name collision aborted the whole registration | `~/.claude/skills/` | all 129 skills gone; one warning line in the launch banner |

All three were the same root problem: the requirement was never written down, so
each publication path was built by guess and drifted on its own. Keep this
document accurate before changing a publication path, and add a row when a
harness is added.

**Verified on:** Claude Code 2.1.247, GitHub Copilot CLI 1.0.83-3, on Linux,
2026-09-03.

## Summary

| Harness | Has skills? | Global root | Project root | Namespaced? | Name comes from | Collision behaviour | Verification command |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **claude** | yes | `~/.claude/skills/<name>/SKILL.md` | `<cwd>/.claude/skills/` | plugin skills only, as `plugin:skill` | **directory name** | managed > personal > project; a user-owned skill silently beats a plugin skill of the same name | `claude plugin details <plugin>`; `claude --debug-file F -p ''` then grep |
| **copilot** | yes | `~/.copilot/skills/`, `~/.agents/skills/` | `.github/skills/`, `.agents/skills/`, `.claude/skills/` | no — one flat namespace | **frontmatter `name`**, directory as fallback | project > personal > plugin > builtin, silently | `copilot skill list --json` (assert on `path`, not just `name`) |
| **codex** | yes | `~/.agents/skills/`, `$CODEX_HOME/skills/` | `<dir>/.agents/skills/`, `<project>/.codex/skills/` | no | directory containing `SKILL.md` | duplicates all stay in the catalog; a bare `$name` then resolves to neither (reported, not verified here) | none found |
| **rustyclawd** | yes | `~/.claude/skills/` (Claude-native) | `<cwd>/.claude/skills/` | no | see below — several filename forms | not determined | none found |
| **amplifier** | **undetermined** | — | — | — | — | — | — |

## claude — Claude Code

Skill roots, from the binary's own startup log
(`claude --debug-file F -p ''`, verbatim):

```
Loading skills from: managed=/etc/claude-code/.claude/skills, user=~/.claude/skills, project=[<cwd>/.claude/skills]
Loaded 148 unique skills (managed: 0, user: 124, project: 1, additional: 0, legacy commands: 23)
getSkills returning: 148 skill dir commands, 1 plugin skills, 40 bundled skills, 0 builtin plugin skills
```

* **Layout:** `<dir>/SKILL.md`. Frontmatter is *optional* — `claude plugin
  validate` treats a missing `name` or `description` as a warning, not an error
  (`--strict` promotes warnings to exit 1).
* **The name is the DIRECTORY name.** A `SKILL.md` whose frontmatter says
  `name: amplihack-migrate` in a directory called `migrate` is invoked as
  `migrate`, and the mismatch produces no warning even under `--strict`. This
  is the silent trap on this side.
* **Namespacing:** personal, project and managed skills are bare names. Only
  plugin-supplied skills are `plugin:skill`. Separately,
  `~/.claude/commands/<subdir>/*.md` yields `subdir:name` — that is where
  amplihack's `amplihack:*` entries come from, and they are commands, not
  skills.
* **Collisions are silent**, and a user-owned skill beats a plugin-supplied one
  of the same name (`Total plugin skills loaded: 1 (0 duplicate/user-owned
  entries skipped)`). Being shadowed is therefore *survivable* — being refused
  publication is not, which is why #1449's fix skips rather than aborts.
* **skills-dir plugin:** a directory at `~/.claude/skills/<name>/` containing
  `.claude-plugin/plugin.json` auto-loads next session as `<name>@skills-dir`,
  with its skills under `<name>/skills/<skill>/SKILL.md`. No settings entry, no
  marketplace, no install step. `claude plugin init --help` states this
  verbatim, and it reproduces from scratch. Two hard requirements: `name` must
  be present, and **`author` must be an object** — `"author": "Microsoft"` as a
  bare string fails the whole plugin to load. Dot-prefixed directories are
  never adopted.
* **Verification:** `claude plugin details <plugin>` lists the skills a plugin
  contributes by name; `claude plugin list [--json]` reports each skills-dir
  plugin as loaded or failed; `claude --debug-file F -p ''` prints the scan
  roots, per-source counts, and dedup skips. `/skill-doctor` was removed in
  2.1.247.

**What amplihack does:** publishes each canonical skill as a direct child of
`~/.claude/skills` (`claude_plugin::sync_canonical_skills`), tracked by an
ownership manifest, plus an `amplihack` skills-dir wrapper carrying agents,
commands, context and workflow. Both are driven from `install`, through
`install::claude_publication::CLAUDE_DESTINATIONS`, and again on every launch.

## copilot — GitHub Copilot CLI

Skill roots, from `copilot skill --help`, all confirmed by experiment:

```
Project   .github/skills/, .agents/skills/, or .claude/skills/
Personal  ~/.copilot/skills/ or ~/.agents/skills/
Plugin    Installed plugins that bundle skills
Custom    Directories added with `copilot skill add <directory>`
```

* Copilot reads `.claude/skills` at **project** scope but never
  `~/.claude/skills`. `COPILOT_HOME` relocates `~/.copilot`.
* **Layout:** `<dir>/SKILL.md`, and **frontmatter is mandatory** — a `SKILL.md`
  without it hard-fails and is reported under `✖ The following skills failed to
  load`. `description` is optional.
* **The name is the frontmatter `name`**, falling back to the directory name —
  the exact inverse of Claude Code.
* **No namespacing at all.** Plugin skills are bare names in the same flat
  namespace as personal ones, so a plugin skill can be silently eaten by a
  personal skill of the same name.
* **Precedence:** project > personal > plugin > builtin, and within project
  `.github/skills` > `.agents/skills` > `.claude/skills`. The loser disappears
  from both the text and `--json` output with no warning. Observed directly:
  `copilot plugin install` printed *"Installed 2 skills."* while only one was
  ever discoverable.
* **Plugins:** `plugin.json` at the plugin **root** (not `.claude-plugin/`),
  with `skills/<name>/SKILL.md`. Installed via `copilot plugin install`, or
  declared in `enabledPlugins` in `~/.copilot/settings.json`. There is no
  `~/.copilot/skills/<name>/plugin.json` auto-adoption equivalent to Claude's
  skills-dir mechanism. Session-only `--plugin-dir` plugins load but their
  skills are *not* enumerated by `skill list`.
* **Verification:** `copilot skill list --json` gives `name`, `source`, `path`
  and `enabled`. Because shadowing keeps the name and changes the winner,
  **assert on `path`**, not on the name alone.

## codex — OpenAI Codex CLI

Verified against the shipped native binary of `@openai/codex` 0.150.1
(`node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex`).
The binary carries its own source paths, so `strings` names the module that
does the work.

* **Skills exist and are first-class.** The binary contains
  `ext/skills/src/host_roots.rs`, `ext/skills/src/loader/…`, and skill catalog
  telemetry (`codex.thread.skills.enabled_total`, `skills.executor.catalog_root`).
* **Roots verified present as literals:** `.agents/skills`, `.agents`,
  `skills/.system`, and — from the binary's own skill-authoring instructions,
  verbatim — *"Installs into `$CODEX_HOME/skills/<skill-name>` (defaults to
  `~/.codex/skills`)"*. The current user root is reported to be
  `~/.agents/skills`, with `$CODEX_HOME/skills` kept for backward
  compatibility; that ordering was read from upstream source, **not** verified
  here. `/etc/codex/skills` does **not** appear in this build.
* **Layout:** a directory containing `SKILL.md` — the same shape amplihack
  already emits.
* **No config key adds a skills search directory.** `[skills]` config toggles
  known skills (`[[skills.config]]` by `path` or `name`) and budgets; it cannot
  register a new root. Reported from upstream source, consistent with the
  config keys visible in the binary. So amplihack cannot point Codex at
  `~/.amplihack/.claude/skills`; it would have to copy into a real root.
* **Collisions:** reported that duplicates all remain in the catalog and a bare
  `$name` mention resolves to *neither* when two skills share a name
  (`ext/skills/src/selection.rs`, which the binary does reference).
  **Not verified here** — it needs a live Codex run.
* **Verification:** no skills-listing subcommand was found; `codex doctor` does
  not cover skills. **Undetermined** whether a non-interactive check exists.

**What amplihack does:** nothing. No skills are published for Codex.

## rustyclawd

Verified against the installed `rusty` 0.1.1 binary (`~/.cargo/bin/rusty`),
which likewise carries its own source paths.

* RustyClawd is **`.claude`-native**: the binary contains `.claude/skills`,
  `.claude/skills/`, `crates/tools/src/skill_discovery.rs`,
  `crates/tools/src/skill.rs`, and the tool description *"Name of the skill
  (loads from `.claude/skills/{skill}.md`)"*.
* **Several filename forms are accepted:** the literals `/SKILL.md`,
  `/skill.md`, `skill.yaml`, `skill.yml` and `${CLAUDE_SKILL_DIR}` are all
  present. Which form applies at *user* level versus *project* level could not
  be determined from strings alone. It has been reported that user-level
  candidates are `{s}.md`, `{s}/skill.md`, `{s}.yaml`, `{s}/skill.yaml` — i.e.
  lower-case only, so an upper-case `SKILL.md` would not be found there.
  **Unverified; it needs a live run or the upstream source.** If true,
  amplihack's user-level skills are invisible to `rusty`.
* No skills-listing command was found. Collision behaviour: **undetermined**.

**What amplihack does:** `commands/rustyclawd.rs` routes through
`launch::run_launch("claude", …)`, so rustyclawd inherits the Claude
publication path — including a verification that checks the Claude layout.

## amplifier

**Undetermined, and deliberately so.** `amplifier` is not installed on the
machine this was written on, so nothing here was observed. It is reported to be
a separate third-party Python CLI (`microsoft/amplifier`) whose extension unit
is a bundle registered with `amplifier bundle register --path`, with no
`SKILL.md` discovery. `crates/amplihack-cli/src/bootstrap.rs` installs it with
`uv tool install git+https://github.com/microsoft/amplifier`, which is
consistent with that, but consistency is not verification.

**What amplihack does:** `amplihack-launcher/src/amplifier.rs` has
`ensure_bundle_registered` and `ensure_and_build`, and
`amplihack-launcher/src/codex.rs` has its own `configure_codex` and
`ensure_and_build`. Nothing outside those two files calls any of them — the
only cross-module reference is `pub use amplifier::AmplifierInfo` in
`amplihack-launcher/src/lib.rs`. They are dead code.

## Rules that follow from the table

1. **A successful copy is not evidence of discovery.** Every one of #1344,
   #1438 and #1449 printed a success line. Any publication path must end by
   asking the destination — or the harness — what it can actually see.
2. **Provenance, not the name, decides whether a destination may be replaced.**
   After a successful publish every skill name "collides"; the question is
   whether amplihack wrote what is there (#1449).
3. **Never make one destination's problem every destination's problem.** Skip
   what cannot be published safely, publish the rest, and name what was
   skipped.
4. **The name a harness uses is not portable.** The same directory is `migrate`
   in Claude Code and `amplihack-migrate` in Copilot when the frontmatter and
   the directory disagree. Keep them equal.
