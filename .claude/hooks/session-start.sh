#!/usr/bin/env bash
# SessionStart hook: emit a small additionalContext block with workspace
# state. Output is capped at 10,000 chars by Claude Code; we stay far under
# that. Errors are swallowed so a session never fails to start.

set -u

cd "${CLAUDE_PROJECT_DIR:-.}" 2>/dev/null || exit 0

branch=$(git branch --show-current 2>/dev/null || echo "(no git)")
short_status=$(git status --short 2>/dev/null | head -20)
ahead_behind=$(git rev-list --left-right --count "@{u}...HEAD" 2>/dev/null || echo "0	0")
ahead=$(printf '%s' "$ahead_behind" | awk '{print $2}')
behind=$(printf '%s' "$ahead_behind" | awk '{print $1}')

# Build context as a single string. Use factual statements (recommended by
# Anthropic docs for additionalContext - imperative phrasing trips
# prompt-injection defenses).
context="Current branch: ${branch}"
[ -n "${ahead}" ] && [ "${ahead}" != "0" ] && context+=$'\n'"Local is ${ahead} commit(s) ahead of upstream."
[ -n "${behind}" ] && [ "${behind}" != "0" ] && context+=$'\n'"Local is ${behind} commit(s) behind upstream."
if [ -n "${short_status}" ]; then
    file_count=$(printf '%s\n' "${short_status}" | wc -l | tr -d ' ')
    context+=$'\n'"Uncommitted changes (${file_count} file(s)):"$'\n'"${short_status}"
else
    context+=$'\nWorking tree is clean.'
fi

# Emit JSON for additionalContext. jq isn't guaranteed on the dev's path,
# so build the JSON by hand. Escape backslashes, double-quotes, and
# newlines for valid JSON.
escaped=$(printf '%s' "${context}" \
    | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' \
    | awk 'BEGIN{ORS="\\n"} {print}')
# Drop the trailing literal \n that awk appended.
escaped="${escaped%\\n}"

printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "${escaped}"
