# Build lbug's C++ library once per host

amplihack's memory store uses the `lbug` crate. Its build script compiles the
lbug C++ library, about 2,000 files, with CMake inside the target directory.
sccache does not see that build, and each checkout has its own target
directory. So every new worktree, and every parallel agent, compiled the same
unchanged library again. On 8 November 2026 that put 48 C++ compilers on one
72-core host at once.

`scripts/lbug-prebuilt.sh` builds the library once per host, for the lbug
version in `Cargo.lock`, and every build links that copy instead.

## Use it

Build the shared copy once on each host:

```sh
scripts/lbug-prebuilt.sh build
```

After that, the pre-push and pre-commit cargo runner, `scripts/git-hook-cargo.sh`,
uses it automatically. It never starts the build itself, so a hook stays fast
and tests that run it under a throwaway home build nothing. For your own builds:

```sh
eval "$(scripts/lbug-prebuilt.sh env | sed 's/^/export /')"
cargo build
```

On a host where many agents build amplihack, set it once for every cargo
build, whatever starts it, in `~/.cargo/config.toml`:

```toml
[env]
LBUG_LIBRARY_DIR = "<the LBUG_LIBRARY_DIR that scripts/lbug-prebuilt.sh env printed>"
LBUG_INCLUDE_DIR = "<the LBUG_INCLUDE_DIR it printed>"
```

These name one lbug version. When `Cargo.lock` moves to another lbug version,
`amplihack-memory`'s build script stops the build with a message naming both
versions. Run the script again in that checkout and update the two lines.

The first call builds the library, which takes a few minutes. Later calls, in
any checkout on the same lbug version, return at once. Several builds that
start together wait for one CMake build; they do not each run their own.

## How it works

The lbug crate links a prebuilt library when `LBUG_LIBRARY_DIR` and
`LBUG_INCLUDE_DIR` are both set. In that case its build script skips CMake and
compiles only its small C++ bridge. The script:

- reads the lbug version from `Cargo.lock` and uses the crate source that cargo
  has already downloaded. Nothing is copied into this repository.
- builds it with the crate's own CMake switches, in release mode and as
  position-independent code.
- keeps the result under `~/.cache/amplihack/lbug/<version>-<key>/`. The key
  covers the version, the Rust target and the C++ compiler, so a checkout on
  another lbug version never links the wrong library.
- gathers into one include directory the headers that the crate's bundled build
  uses, and writes an `lbug.hpp` that includes the same headers as the crate's
  bundled branch. The published crate leaves out lbug's single-file header
  generator.

If the script fails or is turned off, lbug builds the normal way.

## Settings

| Variable | Effect |
| --- | --- |
| `AMPLIHACK_LBUG_PREBUILT=0` | Turn it off; lbug builds the normal way |
| `AMPLIHACK_LBUG_CACHE` | Cache root (default `~/.cache/amplihack/lbug`) |
| `AMPLIHACK_LBUG_JOBS` | Parallel jobs for the one build (default: all cores) |

The prebuilt library is a release build. A debug build of amplihack therefore
links release-mode lbug. Rust code, and the crate's own bridge, still build in
debug mode.

## Remove a cache entry

Delete its directory under `~/.cache/amplihack/lbug/`. The next build makes it
again.
