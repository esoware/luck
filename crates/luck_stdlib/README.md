# luck_stdlib

Luau's standard library and Roblox's API, built from Luau definition files, so tools know which globals exist and what a call resolves to.

## Overview

A `StdlibLibrary` holds each function's overloads, along with value shapes and deprecations.

Luau's library is built in, and so is Roblox's API at each `SecurityLevel`. `Stdlib::layer` adds your own definition file on top. `luau_definitions` and `roblox_definitions` return the built-in files as text, for the type checker.

## Definition files

A definition file is named `*.d.luau` and declares what a host gives a script, with Luau's `declare` syntax:

```luau
declare extern type Signal with
    function Connect(self, callback: () -> ())
end

declare app: { started: Signal }
declare function wait(seconds: number?): number
```

A project lists them under `definitions` in `luck.json`. They load in order, on top of Luau's own library:

```json
{
  "definitions": ["@roblox", "types/app.d.luau"],
  "security": "PluginSecurity"
}
```

`@roblox` is Roblox's API, at the `security` level the project sets. The linter, `luck check`, and the language server all read the same list.

## Usage

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

- `luau_library` returns Luau's library.
- `roblox_library` returns Luau's library with Roblox's API on top.
- `Stdlib` is a library handle that clones share, with `luau`, `with_roblox`, `layer`, and `library`.
- `StdlibLibrary` holds every entry a library declares.
- `StdlibEntry`, `StdlibNamespace`, `StdlibFunction`, `StdlibSignature`, `StdlibParam`, `StdlibArgKind`, `StdlibValue`, `StdlibShape`, and `StdlibDeprecation` describe an entry.
- `StdlibMap` looks entries up by name.
- `analyze` builds a chunk's semantic analysis against a library.
- `SemanticAnalysis` is that analysis, with `is_known_global`, `resolve_callee`, and `chain_callees`.
- `ResolvedCallee` and `ChainCallees` are what a call resolves to.
- `SecurityLevel` is `None`, `LocalUserSecurity`, `PluginSecurity`, or `RobloxScriptSecurity`.
- `luau_definitions` and `roblox_definitions` return the built-in definition files as text.

## Cargo features

- `schemars` derives `JsonSchema` for `SecurityLevel`, for the `luck.json` schema.
