#!/usr/bin/env bash
# PostToolUse hook (matcher: Edit|Write|MultiEdit). Runs `cargo fmt` on
# the touched .rs file so Rust source stays formatted without prompting
# Claude. Other extensions are ignored. All errors are swallowed -
# formatting failures must not block edits.

set -u

input=$(cat)

# Extract tool_input.file_path. Avoid jq dependency: a small grep+sed
# pipeline handles the JSON shape Claude Code emits.
file_path=$(printf '%s' "${input}" \
    | tr -d '\n' \
    | grep -o '"file_path"[[:space:]]*:[[:space:]]*"[^"]*"' \
    | head -1 \
    | sed -e 's/.*"file_path"[[:space:]]*:[[:space:]]*"//' -e 's/"$//')

# Only format Rust files.
case "${file_path}" in
    *.rs)
        cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0
        cargo fmt -- "${file_path}" >/dev/null 2>&1 || true
        ;;
    *)
        ;;
esac

exit 0
