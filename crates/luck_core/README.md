# luck_core

The types luck's crates share when neither depends on the other: the dialect a user names and the diagnostics a run reports.

## Public surface

- `LuaTarget`: the dialect a user picks, Lua 5.1 to 5.5, Luau, or Luau on Roblox. It parses from names such as `5.4`, `luau`, or `roblox`, and projects onto the two axes the rest of luck reads, `lua_version()` for syntax and `stdlib_environment()` for the standard library.
- `Diagnostic`: an error or warning with a code, a message, a file path, a span, and optional help. Each coded one is built by a numbered constructor, such as `Diagnostic::e008` for a parse failure, so every code is defined in one place.
- `DiagnosticSeverity`: `Error` or `Warning`.

```rust
use luck_core::LuaTarget;
use luck_token::{LuaVersion, StdlibEnvironment};

let target: LuaTarget = "roblox".parse().unwrap();
assert_eq!(target.lua_version(), LuaVersion::Luau);
assert_eq!(target.stdlib_environment(), StdlibEnvironment::Roblox);
```
