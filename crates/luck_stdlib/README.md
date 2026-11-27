# luck_stdlib

Luau's standard library and Roblox's API as data, built from Luau definition
files.

A `StdlibLibrary` describes every global, function, and namespace a script can
reach, with each function's overloads, value shapes, and deprecations. Luau's
library is built in, and so is Roblox's API, filtered to what a script at a
given `SecurityLevel` can access. `Stdlib::layer` adds your own definition file
on top, for a host that gives scripts globals of its own.

`analyze` builds a chunk's scopes and checks its names and calls against a
library, so a tool can ask whether a global exists, or what a call like
`string.format(...)` resolves to.

## Definition files

A definition file, named `*.d.luau`, describes what a host gives its scripts
with Luau's `declare` syntax:

```luau
declare extern type Signal with
    function Connect(self, callback: () -> ())
end

declare app: { started: Signal }
declare function wait(seconds: number?): number
```

In a luck project, `luck.json` lists definition files under `definitions`, and
they load in order on top of Luau's library. `@roblox` stands for Roblox's API,
at the `security` level the project sets:

```json
{
  "definitions": ["@roblox", "types/app.d.luau"],
  "security": "PluginSecurity"
}
```

## Usage

This example layers a host global over Luau's library, then checks a chunk that
uses it:

```rust
use luck_allocator::Allocator;
use luck_stdlib::Stdlib;

let stdlib = Stdlib::luau()
    .layer("declare app: { onStart: () -> () }")
    .expect("the definition parses");

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"print(app)");
let chunk = allocator.alloc(parsed.chunk);
let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, stdlib.library());
assert!(errors.is_empty());
assert!(analysis.is_known_global("print"));
assert!(analysis.is_known_global("app"));
```

## API

* `luau_library`, Luau's library, built the first time a process asks for it.
* `roblox_library`, Luau's library with Roblox's API at a `SecurityLevel`.
* `library_for`, which picks Roblox's library at `SecurityLevel::None`, or
  Luau's alone.
* `Stdlib`, a library handle that clones share, with `luau`, `with_roblox`,
  `layer`, and `library`.
* `StdlibLibrary` and what it holds: `StdlibEntry`, `StdlibNamespace`,
  `StdlibFunction`, `StdlibSignature`, `StdlibParam`, `StdlibArgKind`,
  `StdlibValue`, `StdlibShape`, and `StdlibDeprecation`.
* `StdlibReplacement`, the calls of a deprecated function that its replacement
  can take.
* `StdlibMap`, entries looked up by name.
* `analyze`, which returns a `SemanticAnalysis`, with `is_known_global` and
  `chain_callees`, which resolve to `ResolvedCallee`s and `ChainCallees`.
* `SecurityLevel`, one of `None`, `LocalUserSecurity`, `PluginSecurity`, and
  `RobloxScriptSecurity`.
* `luau_definitions` and `roblox_definitions`, the built-in definition files as
  text, ready for a type checker.

## Cargo features

* `schemars` - Derives `JsonSchema` for `SecurityLevel`, for the `luck.json`
  schema.
