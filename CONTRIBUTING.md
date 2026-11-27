# Contributing

## Setup

You need [rustup](https://rustup.rs), which installs the Rust version `rust-toolchain.toml` pins, [just](https://github.com/casey/just), Node.js for the VS Code extension, the platform's C++ compiler, which `luck_conformance` builds the reference Luau interpreter with, and clang, which `luck_type_checker` builds Luau's type checker with. Then run `just init` once to fetch the Luau submodule and install the extension's packages and the cargo tools the checks use. `just luau` moves the submodule to another Luau release.

Install clang and lld from [LLVM](https://github.com/llvm/llvm-project/releases) at the major version of the LLVM `rustc -vV` prints, which `just release-binary` needs to optimize the Rust and the C++ together, or on macOS, where that recipe doesn't run, use Xcode's clang. A build uses the clang `CXX` names, or else `clang-cl` for an MSVC target and `clang++` for any other, from `PATH` or, on Windows, from `C:\Program Files\LLVM\bin`, where LLVM's installer puts it.

## Checking a change

Run `just ready` before you open a pull request. It formats the code and runs every lint, test, and check CI runs, and CI fails if it leaves a file changed, so commit what it formats.

`just` lists every recipe. `just test` runs the tests and takes nextest's arguments, as in `just test -p luck_parser`.

## Pull requests

Pull requests are squash merged, and release notes and version bumps come from their titles, so a title is `type(scope): subject`, as in `fix(lexer): keep a backslash-escaped CRLF whole in interpolated strings`. The type is one of:

- `feat`: something new a user or a caller can do
- `fix`: a bug fix
- `perf`: a speedup with no change in behavior
- `refactor`: a code change with no change in behavior
- `test`: tests only
- `docs`: docs, comments, and agent instructions only
- `build`: Cargo, npm, extension packaging, and toolchain configuration
- `ci`: GitHub workflows and actions
- `chore`: anything else, such as dependency bumps
- `revert`: undoing an earlier commit

The scope is a crate's directory name without `luck_`, such as `parser` or `lsp`, or `vscode` for `editors/vscode`, `fixtures` for `tests/fixtures`, `deps` for dependency bumps, or `repo` for root files and `.github`. Separate several with commas, as in `fix(parser,formatter): ...`.

Put `!` before the colon when existing users have to change something, such as for a renamed `luck.json` key, CLI flag, or rule, or a breaking change to a published crate's API: `feat(linter)!: ...`.

## License

luck is under the [MIT license](LICENSE), and by opening a pull request you license your contribution under it too. A crate luck depends on needs a license that `deny.toml` accepts, which `just deny` checks, and a third-party file committed to the repository keeps its license text beside it.
