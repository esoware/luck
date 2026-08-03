---
name: bump-versions
description: Applies the workspace-wide lockstep version bump (single shared 0.x version for every publishable crate and the VS Code extension). Use when asked to bump versions, prep a release, or determine what a version should be.
allowed-tools: Read, Edit, Bash(cargo metadata:*), Bash(cargo check:*), Bash(git diff:*), Bash(git status:*), Bash(git log:*), Bash(git tag:*)
---

!`cat "${CLAUDE_PROJECT_DIR}/.agents/skills/bump-versions/SKILL.md"`

Arguments: $ARGUMENTS
