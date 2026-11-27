# Install test isolation and release preflight

This document specifies the change for issue #3416.

## Home resolution

`commands/install/paths.rs::home_dir` reads `HOME` on every call. It rejects
an unset or empty value. It neither consults the password database nor caches
the home. Staging, release allocation, binary links and global settings use
this helper. An environment change between calls can therefore redirect one
install's writes.

`issue_3203_bundle_memory_files::install_stages_home_claude_md_and_no_bundle_memory_file`
uses `issue_527_tests::with_install_env`, which holds the shared environment
lock while the install runs. The competing `issue_3220_directory_index`
fixture had a concrete teardown defect: `InstalledHome` declared its lock
before its environment guard. Rust drops fields in declaration order, so
the lock becomes available before the guard restores `HOME`. That restoration
can overwrite a subsequent fixture's home after it acquires the lock. This
explains a possible route; the historical incident's exact interleaving is
not established.

There is a separate subprocess route.
`no_python_probe_test::probe_script_exits_zero_and_covers_tc04_through_tc08`
starts `scripts/probe-no-python.sh` with the built CLI path and no fixture
home. The script invokes the CLI directly, including `query-code` and
`index-code`, so startup installation inherits the test runner's home. This
route bypasses `tests/support/isolated_amplihack.rs`.

Native Codex installation has the same repeated-home path:
`codex_plugin::root` derives `.amplihack/codex` from `paths::home_dir`, while
`codex_plugin::codex_home` reads `CODEX_HOME` or derives `.codex` from that
helper. Neither lookup caches the home. Its operation scope captures paths
for that operation, rather than the process lifetime. A competing restoration
can therefore affect native package tests too. The precise historical native
plugin interleaving remains unproven. An inherited `CODEX_HOME` is a separate
redirection route; `AMPLIHACK_HOME` is also read first during framework source
selection, so changing `HOME` alone does not isolate source discovery.

## Process boundary

Every test that performs installation or update must run its body in its own
test-binary child with one selected test. The launcher gives that child a
temporary `HOME` through `Command::env`, before it starts. It clears inherited
framework, client-profile, binary-destination, toolchain and recipe-run
overrides, then allows explicit fixture values. The child reuses the compiled
test binary and a system executable search path. The ignored native acceptance
test can receive its explicitly selected executable. Installation fixtures and their children must not inherit a
destination outside their fixture.

Tests that only compute paths, plans or validation results can stay in process.
Any parent-process environment mutation still holds the shared lock until
restoration completes, including panic unwinding. Fix `InstalledHome` by
restoring its environment before releasing its lock. Replace manual restoration
in affected install helpers with existing environment guards.

Shell-based CLI tests need the same explicit home and override isolation as
direct CLI tests. Disabling startup installation is suitable for tests that
do not exercise it; startup-install scenarios must leave it enabled and use
their own complete source bundle and destinations.

## Complete release source

Preflight accepts a source only when all of these conditions hold:

1. Existing bundle compatibility validation succeeds. This retains the
   smart-orchestrator references, all four companion recipe contracts and
   the recipe manifest requirements.
2. `default-workflow.yaml` and `investigation-workflow.yaml` are regular,
   non-symlink recipe files. Each parses as a YAML mapping with its expected
   name and a nonempty sequence of mapping steps with an operation type.
3. The `dev-orchestrator`, `default-workflow` and `investigation-workflow`
   skill directories each contain a regular, non-symlink, nonempty `SKILL.md`.
4. Every source directory in the selected layout's existing `dir_mapping`
   exists as a real directory. Source-conditional directories remain optional;
   when present, they must satisfy the existing source-directory checks.
5. Every file in `required_source_files(layout)` is a regular, non-symlink,
   nonempty file. The first available home-instructions source must meet the
   same file requirement. Its existing lookup order remains
   `context/AMPLIHACK_HOME_CLAUDE.md`, bundle `CLAUDE.md`, then parent
   `CLAUDE.md`.

The checkout and locally available release trees at v1.2.200, v1.2.247 and
v1.2.249 contain the named recipes and skill entrypoints. The orchestrator
routes work to the two supported workflows. These are execution entrypoints,
not incidental catalog names. The older tree demonstrates why the earlier
home-instructions location must remain supported. The existing directory and
file mappings define the remaining framework requirements; modules can contain
documentation, so preflight does not invent a module-code requirement.

An empty directory or a `marker.txt` does not satisfy an execution entrypoint.
Preflight preserves the existing exception for the installed bundle link into
a completed release. Other source-root symlinks remain refused. Recursive
copying continues to skip nested symlinks rather than follow them.

Resolve and preflight the source before creating an installation lock file,
allocating a release, creating staging directories, retiring leftovers or
copying files. Apply the same ordering to implicit installation callers that
hold an outer install lock. Keep the existing under-lock rechecks that prevent
duplicate staging and implicit downgrades. This change gives incomplete input
no installation side effects; it does not promise rollback for unrelated
failures after valid input passes preflight.

## Behavioral evidence

Use disposable fixture and sentinel homes only. A guard coordinates a parent
environment mutation and a child install through pipes or channels. The
parent holds the shared lock while it assigns the sentinel home, starts the
explicitly scoped child and keeps that mutation active through installation.
Exercise removal of the parent's home as a separate case. Assert installed
files exist only in the assigned fixture and that a recursive sentinel
snapshot, including links and file contents, is unchanged. No sleeps, polling
or elapsed-time conditions establish success.

For the negative control, remove the launcher's explicit child home only in
the disposable reproduction. The sentinel case must fail the same guard.
Restore the protection and demonstrate a pass. Record both results without
using an actual home as a target.

Install a complete fixture, seed staged leftovers, and snapshot its releases,
links, settings, instructions and staging files. Attempt installation from a
source without recipes, then from the former recipe-bearing, marker-only
stub without skills. Each attempt must fail and leave the snapshot unchanged.
Also reject incomplete input against an empty fixture without creating an
installation lock or staging tree. A representative complete source must
install successfully. Keep existing install, update, startup and memory-file
regressions, and assert outcomes rather than diagnostic wording.

## Implementation status

`release_preflight::inspect` now checks the source before
`local_install_scoped` acquires its installation lock. It shares the home
instructions lookup with staging. The source fixtures now contain the three
workflow skill entrypoints required by the predicate.

`issue_3416_release_preflight` covers complete sources, recipe-free and
recipe-only stubs, malformed execution assets, instruction precedence and
symlink refusal. Its installation rejection test runs in an isolated child
with an explicit temporary home and no inherited environment overrides. It
compares files, directories and link targets for an empty destination and a
seeded live release with staging leftovers.

Startup self-heal and launch bootstrap now prepare and validate their source
before taking the outer installation lock. Preparation owns any fetched tree
until installation finishes. Both paths retain the stamp and asset rechecks
under the lock, so a completed competing install still suppresses staging.
Startup reads the compatibility memo without writing on a cache miss until
it knows no installation is needed. A fetched source clears an old failure
marker only after validation accepts the tree.

`implicit_rejection_preserves_installation_state` covers both outer-lock
paths against empty and seeded homes, for sources without recipes and stubs
without skills. It uses source acquisition with a selected fixture path and
checks complete destination snapshots, including compatibility-cache and lock
files. Source fetch failures retain the existing compatible-bundle fallback.

`test_support::install_process` now starts selected installation tests in a
fresh test-binary process with the destinations set before startup. Direct
installation, force-refresh, directory-index, memory-file and several repair
regressions use it. Fixture recipe runners are explicit test doubles, so those
tests no longer depend on an executable inherited from the surrounding session.
Workflow skill fixtures include the front matter used by native registration.

`InstalledHome` now drops its environment guard before its shared lock. Both
`with_install_env` helpers use existing environment guards to restore values on
panic as well as normal return. The panic-restoration regression covers both.

`install_child_ignores_competing_home_and_inherited_destinations` starts a child
behind a pipe. The parent holds the environment lock and changes or removes
its own home before releasing the child to install. The child uses the shared
launcher. The test checks installation files in the assigned home and an
unchanged snapshot of the sentinel, including when destination overrides point
at it in the parent. The guard also performs native Codex installation with a
fixture executable. It checks the assigned plugin package, skills, hooks and
configuration, while retaining seeded sentinel plugin and cache paths byte for
byte. Both `AMPLIHACK_HOME` and `CODEX_HOME` point at the sentinel in the parent.
The positive guard uses no timing assumptions.

The implicit-mode installer cases, fetch-backoff installation paths and inherited
profile launch writers also use isolated children. Update tests that inspect
command plans or inject counting closures do not run installation and remain
in process. The native plugin install/update acceptance test uses the same
child launcher and retains its explicit executable input.

The three no-Python shell probes now use a private environment for the shell
and its CLI grandchildren. The two workflow-publish shell paths that expose
the built CLI through `PATH` also use private environments. These graph and
publication tests disable unrelated startup installation. Existing direct CLI
invocations retain their common isolated launcher; first-update and live-run
installation scenarios retain their explicit fixture environments.

The recipe-runner missing-tool fixture now restores originally absent `HOME`
and `CARGO_HOME` values as well as present ones. Both recipe-runner fixtures
that change `HOME` use an environment guard that drops before their shared
lock, including on panic.

The two binary-deployment tests that derive release and link destinations from
`HOME` now use isolated children too. Their home and override guards restore
on panic. The direct home mutations in fleet, mode, help, update-shell and
self-heal tests use restore-on-drop guards under the shared environment lock.
Other fixtures that own both a lock and an environment guard restore before
unlocking. The shared `set_home` helper now returns the existing `HomeGuard`,
so legacy callers also restore on panic before their shared lock drops. Early
restoration through `restore_home` drops that guard. The redundant memory-config
wrapper and the bootstrap test's manual restore closure have been removed.
`shared_home_helper_restores_home_and_profiles_after_panic` checks present and
absent original homes, and restores an inherited profile override as well.

The sentinel guard's negative control replaced the launcher's assigned home
with the competing parent's disposable sentinel home. The same test failed
because `.amplihack/CLAUDE.md` was absent from the assigned fixture. The
launcher source was restored immediately after the experiment. The negative
control used no actual home as an installation destination. With the protection
restored, the guard passed both parent-home cases. The focused run also passed
the binary-deployment, directory-index, self-heal, update, help, mode and fifteen
fleet home cases, for 178 passing tests in total.

The repaired complete installer suite passed 790 tests with one existing ignored
test. The outside-in CLI scenario in
`tests/qa/scenarios/issue-3416-release-preflight.yaml` uses the compiled CLI
and the checkout's release assets. It checks a successful installation before
rejecting recipe-free and marker-only-skill replacements, comparing the live
files, link targets and staged leftovers after each refusal. Ancillary
executables are fixture test doubles; the scenario requires no downloads or
client login. The scenario passed after adding the fixture client required
by native registration. The all-target lint gate also passed.

Broader repository checks and external review remain pending.

Native Codex transaction, upgrade, recovery and probe-install tests now enter
isolated test children before taking their environment locks. The inherited
Codex-home regression uses the same boundary. Pure package planning and path
validation tests retain their explicit disposable path arguments.

All five `fixture_environment` helpers capture and clear the shared profile
and framework-source override set, including `AMPLIHACK_HOME` and `CODEX_HOME`,
and restore it before their callers release the lock. The shared `HomeGuard`
already clears that set. Regression tests now exercise both overrides through
shared-home panic restoration and every installer fixture helper.

The native Codex installer suite passed 215 tests after this isolation change.
Six external upgrade and recovery cases and the inherited-home regression
also passed, as did the four isolation and restoration guards.
