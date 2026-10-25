#!/usr/bin/env bash
# The checkpoint must not turn a Python/model edit into a Rust-wide formatting commit.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd -P)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/checkpoint-scoped-format-XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

awk '
  /^  - id: "checkpoint-after-implementation"$/ { step = 1; next }
  step && /^  - id: / { exit }
  step && /^    command: \|$/ { command = 1; next }
  command {
    if (substr($0, 1, 6) == "      ") { print substr($0, 7); next }
    if ($0 ~ /^[[:space:]]*$/) { print ""; next }
    exit
  }
' "$ROOT/amplifier-bundle/recipes/workflow-tdd.yaml" > "$WORK/checkpoint.sh"
test -s "$WORK/checkpoint.sh"

mkdir -p "$WORK/bin" "$WORK/main/src"
cat > "$WORK/bin/amplihack" <<'SH'
#!/usr/bin/env bash
test "$*" = 'hygiene artifact-guard --repo . --mode pre-publish'
SH
chmod +x "$WORK/bin/amplihack"

git -C "$WORK/main" init -q
git -C "$WORK/main" config user.name 'Checkpoint Tester'
git -C "$WORK/main" config user.email 'checkpoint@example.com'
cat > "$WORK/main/Cargo.toml" <<'TOML'
[package]
name = "checkpoint-scope-fixture"
version = "0.1.0"
edition = "2021"
TOML
printf 'pub fn value( )->i32{ 1 }\n' > "$WORK/main/src/lib.rs"
git -C "$WORK/main" add Cargo.toml src/lib.rs
git -C "$WORK/main" commit -qm initial
git -C "$WORK/main" worktree add -qb task "$WORK/task"

checkpoint() {
  WORKTREE_SETUP_WORKTREE_PATH="$WORK/task" REPO_PATH="$WORK/task" \
    AMPLIHACK_HOME="$ROOT" PATH="$WORK/bin:$PATH" \
    bash "$WORK/checkpoint.sh" > "$WORK/stdout" 2> "$WORK/stderr"
}
fail() {
  cat "$WORK/stdout" "$WORK/stderr" >&2
  echo "FAIL: $*" >&2
  exit 1
}

# A non-Rust task succeeds even when pristine Rust files fail rustfmt.
printf 'def answer():\n    return 42\n' > "$WORK/task/model.py"
printf '%s\n' '---- MODULE Model ----' '====' > "$WORK/task/Model.tla"
before="$(git -C "$WORK/task" rev-parse HEAD)"
checkpoint || fail 'Python/TLA checkpoint failed'
test "$(git -C "$WORK/task" show --format= --name-only HEAD | sort)" = $'Model.tla\nmodel.py' \
  || fail 'non-Rust checkpoint committed unexpected paths'
test "$(cat "$WORK/task/src/lib.rs")" = 'pub fn value( )->i32{ 1 }' \
  || fail 'non-Rust checkpoint changed pristine Rust'
test "$(git -C "$WORK/task" rev-parse HEAD)" != "$before" \
  || fail 'non-Rust checkpoint did not commit'

# A Rust edit invokes a read-only check. Failure leaves the edit in place.
printf 'pub fn value( )->i32{ 2 }\n' > "$WORK/task/src/lib.rs"
before="$(git -C "$WORK/task" rev-parse HEAD)"
if checkpoint; then fail 'unformatted Rust passed'; fi
test "$(git -C "$WORK/task" rev-parse HEAD)" = "$before" \
  || fail 'format failure created a commit'
test "$(cat "$WORK/task/src/lib.rs")" = 'pub fn value( )->i32{ 2 }' \
  || fail 'format check modified the intentional Rust edit'

# After explicit formatting, the Rust edit is checkpointed.
rustfmt "$WORK/task/src/lib.rs"
checkpoint || fail 'formatted Rust checkpoint failed'
test "$(git -C "$WORK/task" show --format= --name-only HEAD)" = 'src/lib.rs' \
  || fail 'Rust checkpoint committed unexpected paths'

# A rename needs both pathspecs; a literal bracketed name must not expand.
printf 'old\n' > "$WORK/task/old.txt"
printf 'bracketed\n' > "$WORK/task/file[1].txt"
printf 'plain\n' > "$WORK/task/file1.txt"
git -C "$WORK/task" add old.txt 'file[1].txt' file1.txt
git -C "$WORK/task" commit -qm 'fixture for rename and literal pathspec'
git -C "$WORK/task" mv old.txt new.txt
printf 'changed\n' > "$WORK/task/file[1].txt"
checkpoint || fail 'rename and literal path checkpoint failed'
paths="$(git -C "$WORK/task" show --format= --name-only --no-renames HEAD | sort)"
test "$paths" = $'file[1].txt\nnew.txt\nold.txt' \
  || fail 'rename or literal pathspec checkpoint included the wrong files'

# A file that moves from staged to unstaged during validation changes scope.
# Its path appears in both snapshots, so the Git state boundaries matter.
printf 'pub fn value() -> i32 {\n    3\n}\n' > "$WORK/task/src/lib.rs"
git -C "$WORK/task" add src/lib.rs
cat > "$WORK/bin/cargo" <<'SH'
#!/usr/bin/env bash
git reset -q -- src/lib.rs
SH
chmod +x "$WORK/bin/cargo"
before="$(git -C "$WORK/task" rev-parse HEAD)"
if checkpoint; then fail 'staged-to-unstaged transition passed'; fi
grep -q 'checkpoint scope changed before staging' "$WORK/stderr" \
  || fail 'staged-to-unstaged transition did not explain the failure'
test "$(git -C "$WORK/task" rev-parse HEAD)" = "$before" \
  || fail 'staged-to-unstaged transition created a commit'
test "$(git -C "$WORK/task" status --short -- src/lib.rs)" = ' M src/lib.rs' \
  || fail 'validation did not unstage the captured file'

# Even a tool that creates a new path after the snapshot cannot extend it.
printf 'pub fn value() -> i32 {\n    3\n}\n' > "$WORK/task/src/lib.rs"
cat > "$WORK/bin/cargo" <<'SH'
#!/usr/bin/env bash
printf 'pub fn stray() {}\n' > stray.rs
SH
chmod +x "$WORK/bin/cargo"
before="$(git -C "$WORK/task" rev-parse HEAD)"
if checkpoint; then fail 'scope expansion passed'; fi
grep -q 'checkpoint scope changed before staging' "$WORK/stderr" \
  || fail 'scope expansion did not explain the failure'
test "$(git -C "$WORK/task" rev-parse HEAD)" = "$before" \
  || fail 'scope expansion created a commit'
test -f "$WORK/task/stray.rs" || fail 'scope failure discarded the new file'

rm "$WORK/task/stray.rs"
cat > "$WORK/bin/cargo" <<'SH'
#!/usr/bin/env bash
printf 'stray\n' > staged-stray.txt
git add staged-stray.txt
rm staged-stray.txt
SH
chmod +x "$WORK/bin/cargo"
if checkpoint; then fail 'staged-only scope expansion passed'; fi
grep -q 'checkpoint scope changed before staging' "$WORK/stderr" \
  || fail 'staged-only scope expansion did not explain the failure'
test "$(git -C "$WORK/task" rev-parse HEAD)" = "$before" \
  || fail 'staged-only scope expansion created a commit'
git -C "$WORK/task" diff --cached --name-only | grep -qx 'staged-stray.txt' \
  || fail 'staged-only scope failure discarded the staged file'

echo 'PASS: checkpoint formats read-only and stages only captured paths'
