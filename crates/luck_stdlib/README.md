# luck_stdlib

The standard library luck reads, built from Luau definition files: Luau's own, with the definitions a project loads layered over it, such as Roblox's API or a host application's, and what a chunk's names and calls reach in it.

## Overview

A `StdlibLibrary` is a library's globals and the shapes of its values, with signatures, overloads, and deprecations, and whether Roblox's API is among its layers, which the Roblox rules ask. `luau_library` returns Luau's, built from `stdlib_data/luau.luau` the first time a process asks for it, and `roblox_library` Luau's with Roblox's API at a security level over it, built once per level. Nothing is generated ahead of time: in an optimized build, Luau's takes about 0.2 ms to build and Roblox's about 5 ms, most of it deriving the level's text and parsing it.

A `Stdlib` is a library a clone shares across threads. `Stdlib::luau` is Luau's, `with_roblox` layers Roblox's API at a level over one, and `layer` layers a definition file over one, in an arena of its own, sharing every entry it leaves alone with the library below.

`analyze` builds a chunk's `luck_semantic::Semantic`, with its node table and `require` sites, and reads it against a library, so a caller can ask whether a name is a known global or which function a call reaches. A local initialized from a value of known shape, such as `local s = string.rep(...)`, has that shape, so `s:upper()` resolves too. It lives apart from `luck_semantic`, so a pass that needs only scoping, such as the minifier's, builds no library.

## Definition files

A definition file is Luau's declaration syntax, which `luck_parser::parse_definitions` reads. A project lists Roblox's API, as `@roblox`, and its own definition files in `luck.json`'s `definitions`, and they layer, in order, over Luau's library. What a file declares replaces what a library below declares under the same name, as Luau's `loadDefinitionFile` treats a redeclared global, so the linter reads a project's definitions as `luck check` does, and `typeof` of a global reads the one declared before it, so `declare string: typeof(string) & { trim: (s: string) -> string }` adds `string.trim`.

```luau
type Protocol = "tcp" | "udp"

declare extern type Socket with
    read IsOpen: boolean
    function Send(self, data: string, ...: any): ()
end

declare app: {
    net: {
        connect: (host: string, protocol: Protocol?) -> Socket,
        open: @[deprecated {use = "app.net.connect", reason = "open was renamed"}] (host: string) -> Socket,
    },
    onMessage: (text: string) -> (),
}
```

A declaration means what it means in Luau, and nothing more:

- `declare name: T` is a namespace of its fields when `T` is a table type, a function when it's a function type, and otherwise a value of the shape `T` names, an extern type or `string`, through `?`, a union with `nil`, and `type` aliases. `typeof(name)` is the global declared before it, and an intersection has what each part has: the overloads of two function types, the fields of two tables, and a shape's members besides a table's fields, as each `Enum.Material` is an `Enum` with its items.
- `declare function` is a function, and a `declare extern type` a shape, whose `function`s with `self` are methods a colon calls, a method declared again an overload, with its supertype's members under its own.
- A function's parameters are its named ones, each optional when its type takes `nil`, as Luau's `isOptional` says: `T?`, `nil`, `any`, `unknown`, or a union with one of them, and a call can leave out only the optional ones after the last required one. A trailing `...T` or generic pack is a vararg, a union of string singletons, directly or through an alias, the constants it allows, a generic anything, and any other type a label of it as written. A function's call has the shape of its first result's type.
- `@deprecated` or `@[deprecated {use = "...", reason = "..."}]` deprecates a function, which the `deprecated` rule reports with the reason, and fixes, as a suggestion, by putting `use` in the callee's place, when it's a method's name for a method call or a dotted path for any other.
- A declared global is read-only to the linter, and a field or property only when it's declared `read`, as Luau types it.
- Each function of the `string` namespace that takes a string first is also a method of the `string` shape, since a string's metatable indexes the namespace.

A definition file that doesn't parse is an E008 at each place in it.

## Luau's definitions

`stdlib_data/luau.luau` is Luau's standard library as a definition file, which `just stdlib` generates from two sources: Luau's built-in definitions, the text `luck_type_checker::builtin_definitions` gives from the Luau release the submodule pins, and Luau's library reference, `src/content/docs/reference/library.md` in luau-lang/site. Its declarations are the built-in definitions, with a declaration from the reference's signature for each function it documents that they leave to Luau's C++, such as the `string` library, `pairs`, `next`, `getmetatable`, `setmetatable`, `table.clone`, and `table.freeze`. A few of the reference's types are written for the reader, and each becomes the type Luau's checker binds: `table` is `{}`, `function` any function, `userdata` `any`, `<iterator>` any values, `(A, B)?` as `(A?, B?)`, and a parameter `name: ...T` a vararg. The comment above each declaration the reference documents is its paragraph there, and a function the reference calls deprecated is `@[deprecated]`, with its advice as the `reason` and a `use` only when the advice names a function. A test fails when the built-in definitions move past the file, as `just luau` moves them. Both sources are under the MIT license, (c) Roblox Corporation and Lua.org, whose text is in `stdlib_data/LICENSE-luau`.

## Roblox's definitions

`stdlib_data/roblox.luau` is Roblox's API as a Luau definition file, which Luau's `loadDefinitionFile` and `luck_parser::parse_definitions` read, for a type checker to load over Luau's built-in definitions. `just roblox-api` generates it from Roblox's API dump, which gives the classes, their members, each member's security, and the enums, from the RobloxGlobals section of `LuauTypes.d.luau`, the globals Roblox Studio declares for a script, which it carries over as they are, and from the engine reference of the Roblox Creator Documentation, which gives the datatypes, the other globals and libraries Luau's own definitions lack, and each item's summary, as a comment on the line above it. The dump, the full dump it reads deprecated members' preferred members from, and `LuauTypes.d.luau` come from Roblox-Client-Tracker, which extracts them from Studio. Where `LuauTypes.d.luau` types a global `any` that the documentation types as a class, such as `game`, the class stands, as Studio's reflection gives it. It declares classes, datatypes, and the type of each enum's items as `declare extern type`, each enum as a member of `Enums`, the type of the `Enum` global, whose items are the fields of a table, globals and libraries as `declare`, a library Luau declares too as `typeof` it with the members Roblox adds, an event or a property a script can't write `read`, and a callback `write`. A function or method the dump or the documentation deprecates, Studio's globals among them, is `@[deprecated]`, with the documentation's deprecation message as the `reason`, and as the `use` the method the full dump's `PreferredDescriptorName` names, when the class has it, or the one function a global's message names. A parameter is optional when the dump or the documentation gives it a default, or when its documentation summary starts by calling it optional, as `WaitForChild`'s "An optional time out parameter." does, and every parameter after it is optional too. A definition file can't name a type with a dot, so the type of an enum's items is `Enum` and the enum's name, as `EnumMaterial`. A checker types what Studio types past its definitions only when asked, as `luck check` asks `luck_type_checker` to: a call of those three returns the class its string names, and each enum's type is `Enum.Material`.

A script runs at a `SecurityLevel`: `None` for a `Script` or `LocalScript`, `LocalUserSecurity`, `PluginSecurity` for a plugin or the command bar, or `RobloxScriptSecurity` for a `CoreScript`, each reaching what the ones before it reach. The file holds the API once, as a script at `RobloxScriptSecurity` sees it, leaving out what needs a stronger level, such as `RobloxSecurity`. A member that needs more than `None` ends with a comment naming its level, as `-- PluginSecurity`, and a property that needs more to write than to read names the level that writes it, as `-- write: PluginSecurity`. `roblox_definitions` derives a weaker level's text from it in one pass over its lines, leaving out each member above the level, with its summary, and making `read` each property the level can't write. The result is plain definition text, and `RobloxScriptSecurity`'s is the file itself.

The summaries, datatypes, deprecation messages, and the other globals and libraries are adapted from the Roblox Creator Documentation, (c) Roblox Corporation, under CC BY 4.0, whose text is in `stdlib_data/LICENSE-creator-docs`.

## Usage

- `luau_library()` returns Luau's library, and `roblox_library(security)` Luau's with Roblox's API at `security` over it, each a `&'static StdlibLibrary`.
- `SecurityLevel`: the level a Roblox script runs at. It derives `Deserialize` for the four names Roblox's API dump gives them, which `luck.json` takes, and `JsonSchema` under the `schemars` feature.
- `roblox_definitions(security)` returns Roblox's API for a script at that level, as the definitions a type checker loads over Luau's built-in ones.
- `Stdlib::luau()` is Luau's library, `stdlib.with_roblox(security)` it with Roblox's API layered over it, and `stdlib.layer(text)` it with a definition file layered over it, or the file's E008s.
- `analyze(chunk, counts, library)` returns a `SemanticAnalysis` of a tree, and beside it the semantic checker's errors. `SemanticAnalysis` answers `is_known_global`, `resolve_callee`, and `resolve_stdlib_path`, takes the configured extra globals with `add_extra_globals`, and holds the `Semantic`.

```rust
use luck_allocator::Allocator;
use luck_stdlib::Stdlib;

let stdlib = Stdlib::luau()
    .layer("declare app: { onStart: () -> () }")
    .expect("the definition parses");

let allocator = Allocator::new();
let source = b"local greeting = 1 print(greeting, app)";
let parsed = luck_parser::parse(&allocator, source);
let chunk = allocator.alloc(parsed.chunk);
let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, stdlib.library());
assert!(errors.is_empty());
assert!(analysis.is_known_global("print"));
assert!(analysis.is_known_global("app"));
```
