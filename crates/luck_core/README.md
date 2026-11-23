# luck_core

The types luck's crates share when neither depends on the other: the dialect a user names.

## Usage

- `LuaTarget`: the dialect a user picks, Lua 5.1 to 5.5, Luau, or Luau on Roblox. It parses from names such as `5.4`, `luau`, or `roblox`, and splits into the two values the rest of luck reads, `lua_version()` for syntax and `stdlib_environment()` for the standard library. The `serde` feature derives `Deserialize` for the lowercase names `luck.json` takes, `lua51` to `lua55`, `luau`, and `roblox`, and `schemars` derives `JsonSchema` too.

```rust
use luck_core::LuaTarget;
use luck_token::{LuaVersion, StdlibEnvironment};

let target: LuaTarget = "roblox".parse().unwrap();
assert_eq!(target.lua_version(), LuaVersion::Luau);
assert_eq!(target.stdlib_environment(), StdlibEnvironment::Roblox);
```
