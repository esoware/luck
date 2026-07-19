#!/usr/bin/env bash
# Block hand-edits to the generated VS Code config schema. It must be
# regenerated from the Rust types instead:
#   cargo test -p luck_core regenerate_luckrc_schema -- --ignored
input=$(cat)

file_path=$(printf '%s' "$input" | grep -o '"file_path"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*:[[:space:]]*"//; s/"$//')

case "$file_path" in
  *luckrc.schema.json)
    echo "luckrc.schema.json is generated from the Rust types in luck_core - never hand-edit it. Change the types, then run: cargo test -p luck_core regenerate_luckrc_schema -- --ignored" >&2
    exit 2
    ;;
esac

exit 0
