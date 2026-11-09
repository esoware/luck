# Contributing

## License

luck is under the [MIT license](LICENSE), and by opening a pull request you license your contribution under it too.

A crate luck depends on needs a license that `deny.toml` accepts. `just deny` checks them.

A third-party file committed to the repository keeps its license text beside it.

## Commits and pull requests

PRs are squash merged, so the PR title becomes the commit subject and the PR description becomes the commit body.

```
fix(lexer): keep a backslash-escaped CRLF whole in interpolated strings

In a Luau interpolated string, a backslash before a CRLF escaped only the carriage return, and the line feed after it was read as a separate, unescaped character.

The lexer now consumes the line feed with the carriage return, so the pair is one escaped line break.

Closes #24
```

### Title

The title is `type(scope): subject` and fits in 72 characters.

| Type       | For                                                          |
| ---------- | ------------------------------------------------------------ |
| `feat`     | Something new a user or a caller can do                      |
| `fix`      | A bug fix                                                    |
| `perf`     | A speedup with no change in behavior                         |
| `refactor` | A code change with no change in behavior                     |
| `test`     | Tests only                                                   |
| `docs`     | Docs, comments, and agent instructions only                  |
| `build`    | Cargo, npm, extension packaging, and toolchain configuration |
| `ci`       | GitHub workflows and actions                                 |
| `chore`    | Anything else, such as dependency bumps                      |
| `revert`   | Undoing an earlier commit                                    |

The scope is required. It's a crate's directory name without `luck_`, such as `core`, `parser`, `linter`, or `lsp`, or one of these:

- `vscode` for `editors/vscode`
- `fuzz` for `fuzz`
- `fixtures` for `tests/fixtures`
- `deps` for dependency bumps
- `repo` for root files and `.github`

Separate several scopes with commas and no spaces, as in `fix(parser,formatter): ...`. A revert reuses the scope and subject of the commit it undoes.

Put `!` before the colon when existing users have to change something, such as for a renamed `luck.json` key, CLI flag, or rule, or a breaking change to a published crate's API: `feat(core)!: ...`.

The subject starts with a lowercase imperative verb, such as `add`, and has no period.

### Description

Say what was wrong or missing, then what the change does about it. A few plain sentences are usually enough, and a list works when a change does several separate things.

Trailers go at the end, `Closes #N` before `Co-authored-by:`.
