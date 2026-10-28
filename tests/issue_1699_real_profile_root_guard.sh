#!/usr/bin/env bash
# Issue #1699 — the real-profile-root guard notices when a test run changed
# the user's profile root, and says nothing about anything else.
#
# The guard (scripts/check-real-profile-root.sh) takes `snapshot <file>`
# before the test suite and `compare <file>` after it. It watches every root
# that exists among $AMPLIHACK_CLAUDE_PROFILE_ROOT, the root saved in
# ~/.amplihack/profile-root, ~/.claude-profiles and the default
# ~/.amplihack/claude-profiles. It only reports: exit 0 clean, 1 a
# difference, 2 the scan was impossible or a usage error.
#
# Every case runs against a mktemp -d tree: HOME and the root env var point
# into it, and nothing outside it is read or written.
#
# Under test:
#   1. no change passes;
#   2. an edited file fails and is listed as `changed:file <path>`, also when
#      only its mtime moved;
#   3. an added file fails (`added: <path>`), a removed one fails
#      (`removed: <path>`), a retargeted symlink fails (`changed:link`);
#   4. a root absent at both ends passes with `note: <root> absent`; a root
#      created between snapshot and compare fails (`created during run:`);
#   5. awkward names (an embedded newline, `$(touch pwned)`, `-rf`) are
#      recorded without being run or decoded;
#   6. a root of `/` or of $HOME itself, a usage error, and a missing
#      snapshot file exit 2;
#   7. a changed credential file is named, but neither its contents nor
#      any hash is printed;
#   8. the guard has no write commands (beyond making its snapshot file
#      mode 600), eval or printf %b; the snapshot file is mode 600.
set +e -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GUARD="$ROOT/scripts/check-real-profile-root.sh"

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

if [ ! -f "$GUARD" ]; then
  echo "missing $GUARD"
  exit 1
fi

unset AMPLIHACK_CLAUDE_PROFILE_ROOT
work="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf -- "${work:?}"' EXIT

out="" code=0
# run <home> <root-or-empty> <args...>: runs the guard from inside $work
# with a scrubbed environment; sets $out and $code.
run() {
  local home="$1" root="$2"
  shift 2
  if [ -n "$root" ]; then
    out="$(cd "$work" && env -i PATH="$PATH" HOME="$home" \
      AMPLIHACK_CLAUDE_PROFILE_ROOT="$root" bash "$GUARD" "$@" 2>&1)"
  else
    out="$(cd "$work" && env -i PATH="$PATH" HOME="$home" bash "$GUARD" "$@" 2>&1)"
  fi
  code=$?
}

# fresh <name>: a new HOME and root under $work; sets $home, $root, $snap.
fresh() {
  home="$work/$1/home"
  root="$work/$1/real/profiles"
  snap="$work/$1/snap"
  mkdir -p "$home" "$root/a" "$root/b"
  printf '{"hooks":{}}\n' >"$root/a/settings.json"
  printf '{"hooks":{}}\n' >"$root/b/settings.json"
}

expect() { # expect <label> <want-code>
  if [ "$code" = "$2" ]; then pass "$1 (exit $code)"; else
    fail "$1: want exit $2, got $code: $out"
  fi
}
has() { # has <label> <fixed-string>
  if printf '%s' "$out" | grep -qF -- "$2"; then pass "$1"; else
    fail "$1: missing '$2' in: $out"
  fi
}
lacks() { # lacks <label> <fixed-string>
  if printf '%s' "$out" | grep -qF -- "$2"; then
    fail "$1: unexpected '$2' in: $out"
  else pass "$1"; fi
}

# --- 1. no change ----------------------------------------------------------
fresh clean
run "$home" "$root" snapshot "$snap"
expect "snapshot succeeds" 0
run "$home" "$root" compare "$snap"
expect "no change passes" 0
lacks "clean compare lists nothing changed" "changed:"
lacks "clean compare lists nothing added" "added:"

# --- 2. edited file --------------------------------------------------------
fresh edit
run "$home" "$root" snapshot "$snap"
printf '{"hooks":{"Stop":[]}}\n' >"$root/a/settings.json"
touch -d '+2 seconds' "$root/a/settings.json"
run "$home" "$root" compare "$snap"
expect "an edited file fails" 1
has "the edited file is listed" "changed:file $root/a/settings.json"
lacks "the untouched file is not listed" "$root/b/settings.json"

fresh mtime
run "$home" "$root" snapshot "$snap"
touch -d '+5 seconds' "$root/b/settings.json"
run "$home" "$root" compare "$snap"
expect "an mtime-only change fails" 1
has "the touched file is listed" "changed:file $root/b/settings.json"

# --- 3. added, removed, retargeted -----------------------------------------
fresh add
run "$home" "$root" snapshot "$snap"
printf '{}\n' >"$root/a/settings.json.tmp"
run "$home" "$root" compare "$snap"
expect "an added file fails" 1
has "the added file is listed" "added: $root/a/settings.json.tmp"

fresh remove
run "$home" "$root" snapshot "$snap"
rm -f -- "$root/b/settings.json"
run "$home" "$root" compare "$snap"
expect "a removed file fails" 1
has "the removed file is listed" "removed: $root/b/settings.json"

fresh link
ln -s "$work/link/one" "$root/a/link"
run "$home" "$root" snapshot "$snap"
rm -f -- "$root/a/link"
ln -s "$work/link/two" "$root/a/link"
touch -h -d '+3 seconds' "$root/a/link" 2>/dev/null
run "$home" "$root" compare "$snap"
expect "a retargeted symlink fails" 1
has "the symlink is listed" "changed:link $root/a/link"

fresh nofollow
mkdir -p "$work/nofollow/outside"
printf 'x\n' >"$work/nofollow/outside/f"
ln -s "$work/nofollow/outside" "$root/a/out"
run "$home" "$root" snapshot "$snap"
printf 'y\n' >"$work/nofollow/outside/f"
run "$home" "$root" compare "$snap"
expect "a change behind a symlink is not followed" 0

# --- 4. absent roots -------------------------------------------------------
home="$work/absent/home"
root="$work/absent/real/profiles"
snap="$work/absent/snap"
mkdir -p "$home"
run "$home" "$root" snapshot "$snap"
expect "snapshot of an absent root succeeds" 0
run "$home" "$root" compare "$snap"
expect "a root absent at both ends passes" 0
has "the absent root is noted" "note: $root absent"

home="$work/created/home"
root="$work/created/real/profiles"
snap="$work/created/snap"
mkdir -p "$home"
run "$home" "$root" snapshot "$snap"
mkdir -p "$root/a"
printf '{}\n' >"$root/a/settings.json"
run "$home" "$root" compare "$snap"
expect "a root created during the run fails" 1
has "the created root is named" "created during run: $root"

fresh default
mv -- "$root" "$home/.claude-profiles"
run "$home" "" snapshot "$snap"
printf 'changed\n' >"$home/.claude-profiles/a/settings.json"
touch -d '+2 seconds' "$home/.claude-profiles/a/settings.json"
run "$home" "" compare "$snap"
expect "the .claude-profiles root under HOME is watched without the env var" 1
has "the change under ~/.claude-profiles is listed" "changed:file $home/.claude-profiles/a/settings.json"

fresh amplidefault
mkdir -p "$home/.amplihack"
mv -- "$root" "$home/.amplihack/claude-profiles"
run "$home" "" snapshot "$snap"
rm -f -- "$home/.amplihack/claude-profiles/b/settings.json"
run "$home" "" compare "$snap"
expect "the default .amplihack/claude-profiles root is watched" 1
has "the removal under the default root is listed" "removed: $home/.amplihack/claude-profiles/b/settings.json"

fresh saved
mkdir -p "$home/.amplihack"
printf '%s\n' "$root" >"$home/.amplihack/profile-root"
run "$home" "" snapshot "$snap"
printf '{}\n' >"$root/new.json"
run "$home" "" compare "$snap"
expect "the root saved in ~/.amplihack/profile-root is watched" 1
has "the added file under the saved root is listed" "added: $root/new.json"

fresh relsaved
mkdir -p "$home/.amplihack"
printf 'relative/profiles\n' >"$home/.amplihack/profile-root"
run "$home" "" snapshot "$snap"
expect "a relative saved root is ignored, not an error" 0

fresh dedupe
ln -s "$root" "$home/.claude-profiles"
run "$home" "$root" snapshot "$snap"
printf 'z\n' >"$root/a/settings.json"
touch -d '+2 seconds' "$root/a/settings.json"
run "$home" "$root" compare "$snap"
expect "a root reached twice is still compared" 1
n="$(printf '%s\n' "$out" | grep -c -- 'a/settings.json')"
if [ "$n" = 1 ]; then pass "a root reached twice is reported once"; else
  fail "a root reached twice is reported $n times: $out"
fi

# --- 5. awkward names ------------------------------------------------------
fresh names
nl_name="$root/a/line
break.json"
printf '{}\n' >"$nl_name"
printf '{}\n' >"$root/a/\$(touch pwned)"
printf '{}\n' >"$root/a/-rf"
run "$home" "$root" snapshot "$snap"
expect "snapshot with awkward names succeeds" 0
run "$home" "$root" compare "$snap"
expect "awkward names with no change pass" 0
if [ -e "$work/pwned" ] || [ -e "$root/a/pwned" ] || [ -e "$ROOT/pwned" ]; then
  fail "a file name was executed (pwned exists)"
else
  pass "file names are never executed"
fi
if [ -e "$root/a/-rf" ] && [ -e "$nl_name" ]; then pass "awkward files are left in place"; else
  fail "an awkward file disappeared"
fi
printf 'x\n' >"$nl_name"
touch -d '+2 seconds' "$nl_name"
run "$home" "$root" compare "$snap"
expect "a changed file with a newline in its name fails" 1
if printf '%s' "$out" | LC_ALL=C grep -q $'\x1b'; then
  fail "output carries an escape character"
else pass "output carries no escape characters"; fi

fresh ctrl
esc_name="$root/a/$(printf 'bad\033[31mname')"
run "$home" "$root" snapshot "$snap"
printf '{}\n' >"$esc_name"
run "$home" "$root" compare "$snap"
expect "an added file with an escape in its name fails" 1
if printf '%s' "$out" | LC_ALL=C grep -q $'\x1b'; then
  fail "control characters reach the output: $out"
else pass "control characters are stripped from the output"; fi

# --- 6. impossible scans and usage errors ----------------------------------
fresh slash
run "$home" "/" snapshot "$snap"
expect "a root of / exits 2" 2
run "$home" "$home" snapshot "$snap"
expect "a root equal to HOME exits 2" 2
run "$home" "$root"
expect "no arguments is a usage error" 2
run "$home" "$root" frobnicate "$snap"
expect "an unknown command is a usage error" 2
run "$home" "$root" snapshot
expect "a missing snapshot path is a usage error" 2
run "$home" "$root" compare "$work/slash/never-written"
expect "comparing against a missing snapshot exits 2" 2

# --- 7. credential files ---------------------------------------------------
fresh creds
secret_old="SECRET-TOKEN-old-$$"
secret_new="SECRET-TOKEN-new-$$"
printf '{"token":"%s"}\n' "$secret_old" >"$root/a/.credentials.json"
run "$home" "$root" snapshot "$snap"
if grep -qF -- "$secret_old" "$snap"; then fail "the snapshot holds file contents"; else
  pass "the snapshot holds no file contents"
fi
if stat -c %a -- / >/dev/null 2>&1; then mode="$(stat -c %a -- "$snap")"; else
  mode="$(stat -f %Lp -- "$snap")"
fi
if [ "$mode" = 600 ]; then pass "the snapshot file is mode 600"; else
  fail "the snapshot file is mode $mode, not 600"
fi
chmod 644 -- "$snap"
run "$home" "$root" snapshot "$snap"
if stat -c %a -- / >/dev/null 2>&1; then mode="$(stat -c %a -- "$snap")"; else
  mode="$(stat -f %Lp -- "$snap")"
fi
if [ "$mode" = 600 ]; then pass "a reused snapshot file is made mode 600"; else
  fail "a reused snapshot file stays mode $mode"
fi
printf '{"token":"%s"}\n' "$secret_new" >"$root/a/.credentials.json"
touch -d '+2 seconds' "$root/a/.credentials.json"
run "$home" "$root" compare "$snap"
expect "a changed credential file fails" 1
has "the credential file is named" "$root/a/.credentials.json"
lacks "old contents are not printed" "$secret_old"
lacks "new contents are not printed" "$secret_new"
for content in "$secret_old" "$secret_new"; do
  if command -v sha256sum >/dev/null 2>&1; then
    h="$(printf '{"token":"%s"}\n' "$content" | sha256sum | cut -d' ' -f1)"
  else
    h="$(printf '{"token":"%s"}\n' "$content" | shasum -a 256 | cut -d' ' -f1)"
  fi
  lacks "no hash is printed" "$h"
done
if printf '%s' "$out" | grep -qE '[0-9a-f]{64}'; then
  fail "something hash-shaped is printed: $out"
else pass "nothing hash-shaped is printed"; fi

# --- 8. static rules: report only ------------------------------------------
# The one allowed write besides the snapshot itself: making that file private.
# shellcheck disable=SC2016 # a literal $file, not an expansion
code_lines="$(grep -v '^[[:space:]]*#' "$GUARD" | sed 's/chmod 600 -- "\$file"//')"
if printf '%s\n' "$code_lines" |
  grep -nE '(^|[^[:alnum:]_-])(rm|mv|cp|touch|chmod|chown|truncate|ln|mkdir|rmdir|install|dd)([[:space:]]|$)|sed[[:space:]]+-i'; then
  fail "the guard contains a write command"
else pass "the guard contains no write commands"; fi
if printf '%s\n' "$code_lines" | grep -nE '\beval\b|%b|set -x'; then
  fail "the guard uses eval, printf %b or set -x"
else pass "the guard never evals, decodes with %b or traces"; fi
if printf '%s\n' "$code_lines" | grep -nE '>>?[[:space:]]*"?\$\{?root'; then
  fail "the guard redirects output into a root"
else pass "the guard never redirects into a root"; fi
if printf '%s\n' "$code_lines" | grep -qE 'find[[:space:]]+-P'; then pass "the walk uses find -P"; else
  fail "the walk does not use find -P"
fi

if [ "$fails" -ne 0 ]; then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all real-profile-root guard checks passed"
