---
name: release-publish
description: Brings the tree to a provably releasable state and produces the dependency-ordered publish checklist for the human to run (cargo publish is denied to agents). Use when asked to cut a release, publish luck, ship a version, tag a release, or package the VS Code extension.
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*), Bash(just:*), Bash(git status:*), Bash(git diff:*), Bash(git log:*), Bash(git tag:*)
---

!`cat "${CLAUDE_PROJECT_DIR}/.agents/skills/release-publish/SKILL.md"`

Arguments: $ARGUMENTS
