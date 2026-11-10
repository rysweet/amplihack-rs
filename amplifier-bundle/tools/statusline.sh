#!/bin/bash
# statusline.sh tolerates individual command failures (display-only)
# so pipefail is intentionally not set — partial output is better than no output

# Amplihack Status Line
# Shows: directory, git branch, model, tokens, cost, duration, background agents
#
# Configure in ~/.claude/settings.json:
#   "statusLine": {
#     "type": "command",
#     "command": "$CLAUDE_PROJECT_DIR/.claude/tools/statusline.sh"
#   }

# Read JSON from Claude Code and normalize (remove newlines for reliable parsing)
input=$(cat | tr -d '\n\r')

# Extract values without jq (portable)
extract_json() {
    local key="$1"
    local default="$2"
    local value=$(echo "$input" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | sed "s/.*: *\"\([^\"]*\)\".*/\1/" | head -1)
    if [ -z "$value" ]; then
        value=$(echo "$input" | grep -o "\"$key\"[[:space:]]*:[[:space:]]*[^,}]*" | sed "s/.*: *\([^,}]*\).*/\1/" | tr -d ' ' | head -1)
    fi
    echo "${value:-$default}"
}

# Extract session data
current_dir=$(extract_json "current_dir" "")
[ -z "$current_dir" ] && current_dir=$(extract_json "cwd" "$(pwd)")
model_name=$(extract_json "display_name" "Claude")
model_id=$(extract_json "id" "")
total_cost=$(extract_json "total_cost_usd" "0")
total_duration=$(extract_json "total_duration_ms" "0")
transcript_path=$(extract_json "transcript_path" "")

# Extract session ID from transcript path
extract_session_id() {
    local transcript_path="$1"

    # Return empty if no transcript path
    [ -z "$transcript_path" ] && return

    # Extract session_id from path pattern: .../{session_id}/transcript.jsonl
    # The session ID is a UUID like: d9970265-92fd-4544-ab3c-78309a5ee3a5
    local session_id=$(echo "$transcript_path" | grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' | head -1)

    # Fallback: Try older /sessions/ path pattern
    if [ -z "$session_id" ]; then
        session_id=$(echo "$transcript_path" | sed -n 's|.*/sessions/\([^/]*\)/.*|\1|p')
    fi

    # Fallback: Check CLAUDE_SESSION_ID env var
    if [ -z "$session_id" ] && [ -n "$CLAUDE_SESSION_ID" ]; then
        session_id="$CLAUDE_SESSION_ID"
    fi

    echo "$session_id"
}

# Get session ID
session_id=$(extract_session_id "$transcript_path")

# Change to directory for git
cd "$current_dir" 2>/dev/null || cd "$(pwd)"

# Format directory (~ for home)
display_dir=$(echo "$current_dir" | sed "s|^$HOME|~|")

# Model color (red=Opus, green=Sonnet, blue=Haiku)
case "$model_id" in
    *opus*) model_color="31" ;;
    *sonnet*) model_color="32" ;;
    *haiku*) model_color="34" ;;
    *)
        case "$model_name" in
            *Opus*|*opus*) model_color="31" ;;
            *Sonnet*|*sonnet*) model_color="32" ;;
            *Haiku*|*haiku*) model_color="34" ;;
            *) model_color="37" ;;
        esac
        ;;
esac

# Git info
git_info=""
if git rev-parse --is-inside-work-tree &>/dev/null; then
    branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || git rev-parse --short HEAD 2>/dev/null)
    if [ -n "$branch" ]; then
        # Check for uncommitted changes
        if ! git diff-index --quiet HEAD 2>/dev/null; then
            git_color="33"  # Yellow (dirty)
            dirty_marker="*"
        else
            git_color="36"  # Cyan (clean)
            dirty_marker=""
        fi

        # Get remote name
        remote=$(git remote 2>/dev/null | head -1)
        if [ -n "$remote" ]; then
            git_info=" \033[${git_color}m($branch$dirty_marker → $remote)\033[0m"
        else
            git_info=" \033[${git_color}m($branch$dirty_marker)\033[0m"
        fi
    fi
fi

# Calculate tokens from transcript.
# Counts incrementally: a per-transcript cache entry "INODE OFFSET TOTAL"
# under ${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens/ records
# how far the transcript has been read, so each refresh reads only the new,
# complete lines. Prints digits only; all stderr is discarded.
calculate_tokens() {
    local t="$1"
    local st="" ino="" size="" key="" dir="" entry="" tmp=""
    local a="" b="" c="" d="" off=0 total=0 len=0 out="" add="" bytes="" new=""
    local use_cache=0

    # Accept 0 or a decimal integer of at most 18 digits with no leading
    # zero. Glob checks only: these values are never evaluated before this.
    _ct_num() {
        case "$1" in
            0) return 0 ;;
            '' | 0* | *[!0-9]*) return 1 ;;
        esac
        [ "${#1}" -le 18 ]
    }

    if [ -z "$t" ] || [ ! -f "$t" ]; then
        echo "0"
        return 0
    fi

    st=$(stat -L -c '%i %s' -- "$t" || stat -L -f '%i %z' -- "$t")
    ino=${st%% *}
    size=${st#* }
    if ! _ct_num "$ino" || ! _ct_num "$size" || [ "$size" -eq 0 ]; then
        echo "0"
        return 0
    fi

    # Keep only the digest field.
    key=$(printf '%s' "$t" | sha256sum)
    [ -n "$key" ] || key=$(printf '%s' "$t" | shasum -a 256)
    key=${key%% *}
    [ -n "$key" ] || key=$(printf '%s' "$t" | cksum | awk '{print $1 "-" $2}')
    case "$key" in
        '' | *[!0-9a-f-]*) key="" ;;
    esac
    dir="${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/statusline-tokens"
    # mkdir runs only on first use; the checks below still apply every call.
    if [ -n "$key" ] && { [ -d "$dir" ] || (umask 077; mkdir -p -- "$dir"); } \
        && [ -d "$dir" ] && [ ! -L "$dir" ] && [ -O "$dir" ]; then
        use_cache=1
        entry="$dir/$key"
        tmp="$dir/.$key.$$"
    fi

    if [ "$use_cache" -eq 1 ] && [ -f "$entry" ] && [ ! -L "$entry" ] && [ -O "$entry" ]; then
        IFS=' ' read -r -n 128 a b c d < "$entry"
        if [ -z "$d" ] && _ct_num "$a" && _ct_num "$b" && _ct_num "$c" \
            && [ "$a" = "$ino" ] && [ "$b" -le "$size" ]; then
            off=$b
            total=$c
        fi
    fi

    # size > 0, so nothing left to read means a valid entry covers the file.
    len=$((size - off))
    if [ "$len" -eq 0 ]; then
        echo "$total"
        return 0
    fi

    # One pass over the new bytes. The sentinel X ends the stream so that the
    # last record is always the unterminated remainder, which is not counted.
    out=$(
        {
            tail -c +"$((off + 1))" -- "$t" | head -c "$len"
            printf 'X'
        } | LC_ALL=C awk '
            function tally(r,    m) {
                bytes += length(r) + 1
                while (match(r, re)) {
                    m = substr(r, RSTART, RLENGTH)
                    r = substr(r, RSTART + RLENGTH)
                    sub(/^[^:]*:[[:space:]]*/, "", m)
                    add += m + 0
                }
            }
            BEGIN {
                re = "\"(input_tokens|output_tokens|cache_read_input_tokens|cache_creation_input_tokens)\"[[:space:]]*:[[:space:]]*[0-9]*"
                add = 0
                bytes = 0
            }
            NR > 1 { tally(prev) }
            { prev = $0 }
            END { printf "%.0f %.0f\n", add, bytes }
        '
    )
    add=${out%% *}
    bytes=${out#* }
    if ! _ct_num "$add" || ! _ct_num "$bytes"; then
        echo "$total"
        return 0
    fi
    new=$((total + add))
    if ! _ct_num "$new"; then
        echo "$total"
        return 0
    fi
    total=$new
    off=$((off + bytes))

    if [ "$use_cache" -eq 1 ]; then
        # set -C refuses an existing regular file (or a symlink to one), so a
        # planted file is not overwritten.
        if (
            umask 077
            set -C
            exec 3> "$tmp" || exit 1
            printf '%s %s %s\n' "$ino" "$off" "$total" >&3 || { rm -f -- "$tmp"; exit 1; }
        ); then
            mv -f -- "$tmp" "$entry" || rm -f -- "$tmp"
        fi
    fi

    echo "$total"
} 2>/dev/null

# Format tokens for display
format_tokens() {
    local tokens="$1"

    # Handle zero/empty
    if [ -z "$tokens" ] || [ "$tokens" -eq 0 ] 2>/dev/null; then
        echo ""
        return
    fi

    # Format based on magnitude
    if [ "$tokens" -ge 1000000 ]; then
        # Millions: 1.2M
        echo "$(awk "BEGIN {printf \"%.1f\", $tokens/1000000}" 2>/dev/null || echo "0")M"
    elif [ "$tokens" -ge 1000 ]; then
        # Thousands: 234K
        echo "$(($tokens / 1000))K"
    else
        # Under 1K: show exact number
        echo "$tokens"
    fi
}

# Get and format token count
total_tokens=$(calculate_tokens "$transcript_path")
tokens_formatted=$(format_tokens "$total_tokens")
if [ -n "$tokens_formatted" ]; then
    tokens_str=" \033[36m🎫 $tokens_formatted\033[0m"
else
    tokens_str=""
fi

# Format cost (handle awk variations)
cost_formatted=$(echo "$total_cost" | awk '{printf "%.2f", $1}' 2>/dev/null || echo "0.00")

# Format duration
if [ "$total_duration" -eq 0 ] 2>/dev/null; then
    duration_str=""
else
    duration_seconds=$((total_duration / 1000))
    if [ $duration_seconds -lt 60 ]; then
        duration_str=" ⏱${duration_seconds}s"
    elif [ $duration_seconds -lt 3600 ]; then
        duration_minutes=$((duration_seconds / 60))
        duration_str=" ⏱${duration_minutes}m"
    else
        duration_hours=$((duration_seconds / 3600))
        duration_str=" ⏱${duration_hours}h"
    fi
fi

# Power-steering session counter (invocations for current session)
# Uses session_id in path like lock counter
power_steering_str=""
# Use CLAUDE_PROJECT_DIR to find counter (works in worktrees)
project_dir="${CLAUDE_PROJECT_DIR:-$current_dir}"
if [ -n "$session_id" ]; then
    ps_count_file="$project_dir/.claude/runtime/power-steering/$session_id/session_count"
    if [ -f "$ps_count_file" ]; then
        ps_count=$(cat "$ps_count_file" 2>/dev/null || echo "0")
        if [ "$ps_count" -gt 0 ] 2>/dev/null; then
            power_steering_str=" \033[35m🚦×$ps_count\033[0m"
        fi
    fi
fi

# Lock mode indicator (if active)
# Note: session_id is NOT required for basic lock indicator - only for counter
lock_str=""
# Use CLAUDE_PROJECT_DIR to find lock file (works in worktrees)
lock_flag="$project_dir/.claude/runtime/locks/.lock_active"
if [ -f "$lock_flag" ]; then
    # Lock is active - show basic indicator first
    lock_str=" \033[33m🔒\033[0m"

    # Optionally enhance with counter if session_id available
    if [ -n "$session_id" ]; then
        lock_counter_file="$project_dir/.claude/runtime/locks/$session_id/lock_invocations.txt"
        if [ -f "$lock_counter_file" ]; then
            lock_count=$(cat "$lock_counter_file" 2>/dev/null || echo "0")
            if [ "$lock_count" -gt 0 ] 2>/dev/null; then
                lock_str=" \033[33m🔒×$lock_count\033[0m"
            fi
        fi
    fi
fi

# Background agents indicator
# Detects subagents spawned in current session and shows running/stopped count
# Format: 🤖 green_running / red_stopped (e.g., "🤖 2 / 3" = 2 running, 3 stopped)
# When no running: 🤖 red_stopped only (e.g., "🤖 3" = all 3 stopped)
# Active detection uses file modification time heuristic (modified in last 10 seconds)
# Note: Background bash tasks cannot be reliably detected, only subagents
agents_str=""
if [ -n "$session_id" ]; then
    # Build project path from current_dir (Claude uses hyphenated path)
    project_path=$(echo "$current_dir" | sed 's|^/||; s|/|-|g')
    subagents_dir="$HOME/.claude/projects/-$project_path/$session_id/subagents"

    if [ -d "$subagents_dir" ]; then
        # Count total subagent files
        total_agents=$(ls -1 "$subagents_dir"/agent-*.jsonl 2>/dev/null | wc -l)

        if [ "$total_agents" -gt 0 ] 2>/dev/null; then
            # Count recently active agents (modified in last 10 seconds)
            now=$(date +%s)
            active_agents=0
            for agent_file in "$subagents_dir"/agent-*.jsonl; do
                if [ -f "$agent_file" ]; then
                    file_mtime=$(stat -c %Y "$agent_file" 2>/dev/null || stat -f %m "$agent_file" 2>/dev/null)
                    if [ -n "$file_mtime" ]; then
                        age=$((now - file_mtime))
                        if [ "$age" -lt 10 ]; then
                            active_agents=$((active_agents + 1))
                        fi
                    fi
                fi
            done

            finished_agents=$((total_agents - active_agents))

            # Format: 🤖 🟢running / 🔴stopped
            if [ "$active_agents" -gt 0 ]; then
                agents_str=" 🤖 \033[32m${active_agents}\033[0m / \033[31m${finished_agents}\033[0m"
            else
                # No running agents - just show stopped count in red
                agents_str=" 🤖 \033[31m${finished_agents}\033[0m"
            fi
        fi
    fi
fi

# Output status line
echo -e "\033[32m$display_dir\033[0m$git_info \033[${model_color}m$model_name\033[0m$tokens_str 💰\$$cost_formatted$duration_str$agents_str$power_steering_str$lock_str"
