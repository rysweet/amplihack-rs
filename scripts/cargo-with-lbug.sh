#!/usr/bin/env bash
# Local development cache; see PM328 comment6616249815. Bash 3.2 compatible.
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cargo_program="$(command -v cargo)"
if [ "$cargo_program" = "${AMPLIHACK_LBUG_CARGO_SHIM:-}" ]; then
    cargo_program="${AMPLIHACK_LBUG_REAL_CARGO:?CI dispatcher requires original Cargo}"
fi
export AMPLIHACK_LBUG_CARGO="$cargo_program"
export RUSTUP_AUTO_INSTALL=0
managed=1
build=0
for arg in "$@"; do
    [ "$arg" != -- ] || break
    case "$arg" in
        build | check | test | clippy | nextest | run) build=1 ;;
        --release | -r | --profile | --profile=* | install | bench) managed=0; build=1 ;;
    esac
done
[ "$build" -ne 0 ] || exec "$cargo_program" "$@"
if [ "${AMPLIHACK_LBUG_CACHE:-auto}" != auto ] || \
    [ "${LBUG_SHARED+x}${LBUG_LIBRARY_DIR+x}${LBUG_INCLUDE_DIR+x}" != "" ]; then
    managed=0
fi
if [ "$managed" -eq 1 ]; then
    bootstrap="$(mktemp -d "${TMPDIR:-/tmp}/amplihack-lbug-helper.XXXXXX")" || bootstrap=""
    if [ -n "$bootstrap" ]; then
        trap 'rm -rf -- "$bootstrap"' EXIT
        if CARGO_MANIFEST_DIR="$script_dir/../build-support/lbug-cache" \
            rustc --edition=2024 -C opt-level=1 \
            "$script_dir/../build-support/lbug-cache/main.rs" \
            -o "$bootstrap/helper" >/dev/null 2>&1; then
            # Rust removes its mapped temporary executable on startup.
            exec "$bootstrap/helper" --bootstrap-dir "$bootstrap" "$@"
        fi
        rm -rf -- "$bootstrap"
        trap - EXIT
    fi
    printf 'amplihack lbug: helper unavailable; using bundled source\n' >&2
fi
# cxx-build tracks these flags, unlike upstream's LBUG_* path variables.
# Explicit override values contribute to the stable selection fingerprint.
selection_identity() {
    printf '%s\n' "${LBUG_SHARED+x}:${LBUG_SHARED-}" \
        "${LBUG_LIBRARY_DIR+x}:${LBUG_LIBRARY_DIR-}" "${LBUG_INCLUDE_DIR+x}:${LBUG_INCLUDE_DIR-}"
}
if command -v sha256sum >/dev/null 2>&1; then
    selection="$(selection_identity | sha256sum)"
elif command -v shasum >/dev/null 2>&1; then
    selection="$(selection_identity | shasum -a 256)"
else
    # Missing optional tools must not block source compilation. A changing
    # marker conservatively invalidates any old managed output in this case.
    selection="$RANDOM$RANDOM$RANDOM"
fi
marker="-DAMPLIHACK_LBUG_SELECTION_${selection%% *}=1"
for flag_name in ${!CXXFLAGS@} HOST_CXXFLAGS TARGET_CXXFLAGS; do
    if declare -p "$flag_name" >/dev/null 2>&1; then
        printf -v "$flag_name" '%s %s' "${!flag_name}" "$marker"
        export "$flag_name"
    fi
done
case " ${CXXFLAGS:-} " in
    *" $marker "*) ;;
    *) export CXXFLAGS="${CXXFLAGS:-} $marker" ;;
esac
exec "$cargo_program" "$@"
