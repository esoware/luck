# luck_stdlib

The standard library of each variant luck reads, Luau and Roblox, the std definitions that layer a host's library over one, and what a chunk's names and calls reach in it.

## Overview

A `StdlibLibrary` is a library's globals, the shapes of its values, and its named constant sets, with signatures, overloads, and deprecations. Each variant has a static one built into the crate, which `library_for` returns. `just stdlib` generates them from `stdlib_data/`, whose files are std definitions, by layering each variant's files over an empty library and printing the result, so nothing is parsed or checked at runtime. Luau's is `luau.toml`, and Roblox's layers `roblox_api.toml`, `roblox.toml`, and `roblox_enums.toml` over it. `just roblox-api` refreshes the Roblox services, classes, and enums from the live API dump first, and `roblox.luau`, below.

A `StdlibDefinition` is one std definition file, in the format below, which describes a library such as a host application's or a test framework's. `Stdlib::layer` layers it over a library into one of the same type, in an arena of its own, sharing every entry it leaves alone with the library below. A `Stdlib` is a built-in library, or one with definitions layered over it, which a clone shares across threads.

`analyze` builds a chunk's `luck_semantic::Semantic`, with its node table and `require` sites, and reads it against a library, built in or layered, so a caller can ask whether a name is a known global or which function a call reaches. A local initialized from a value of known shape, such as `local s = string.rep(...)`, has that shape, so `s:upper()` resolves too. It lives apart from `luck_semantic`, so a pass that needs only scoping, such as the minifier's, builds no library.

## Roblox's definitions

`stdlib_data/roblox.luau` is Roblox's API as a Luau definition file, which Luau's `loadDefinitionFile` and `luck_parser::parse_definitions` read, for a type checker to load over Luau's built-in definitions. `just roblox-api` generates it from Roblox's API dump, which gives the classes, their members, each member's security, and the enums, from the RobloxGlobals section of `LuauTypes.d.luau`, the globals Roblox Studio declares for a script, which it carries over as they are, and from the engine reference of the Roblox Creator Documentation, which gives the datatypes, the other globals and libraries Luau's own definitions lack, and each item's summary, as a comment on the line above it. Both the dump and `LuauTypes.d.luau` come from Roblox-Client-Tracker, which extracts them from Studio. Where `LuauTypes.d.luau` types a global `any` that the documentation types as a class, such as `game`, the class stands, as Studio's reflection gives it. It declares classes, datatypes, and the type of each enum's items as `declare extern type`, each enum as a member of `Enums`, the type of the `Enum` global, whose items are the fields of a table, globals and libraries as `declare`, a library Luau declares too as `typeof` it with the members Roblox adds, a deprecated function or method `@deprecated`, an event or a property a script can't write `read`, and a callback `write`. A definition file can't name a type with a dot, so the type of an enum's items is `Enum` and the enum's name, as `EnumMaterial`. A checker types what Studio types past its definitions only when asked, as `luck check` asks `luck_type_checker` to: a call of those three returns the class its string names, and each enum's type is `Enum.Material`.

A script runs at a `SecurityLevel`: `None` for a `Script` or `LocalScript`, `LocalUserSecurity`, `PluginSecurity` for a plugin or the command bar, or `RobloxScriptSecurity` for a `CoreScript`, each reaching what the ones before it reach. The file holds the API once, as a script at `RobloxScriptSecurity` sees it, leaving out what needs a stronger level, such as `RobloxSecurity`. A member that needs more than `None` ends with a comment naming its level, as `-- PluginSecurity`, and a property that needs more to write than to read names the level that writes it, as `-- write: PluginSecurity`. `definitions_for` derives a weaker level's text from it in one pass over its lines, leaving out each member above the level, with its summary, and making `read` each property the level can't write. The result is plain definition text, and `RobloxScriptSecurity`'s is the file itself.

The summaries, datatypes, and the other globals and libraries are adapted from the Roblox Creator Documentation, (c) Roblox Corporation, under CC BY 4.0, whose text is in `stdlib_data/LICENSE-creator-docs`.

## Usage

- `Variant`: the Luau a user names, standalone or on Roblox, which decides the standard library a name resolves against and the definitions a type checker loads, since both read the same syntax. It parses from `standalone` or `roblox`, and `is_roblox()` says whether its standard library is Roblox's. It derives `Deserialize` for those lowercase names, which `luck.json` takes, and the `schemars` feature derives `JsonSchema` too.
- `library_for(variant)` returns the built-in library, a `StdlibLibrary`.
- `SecurityLevel`: the level a Roblox script runs at. It derives `Deserialize` for the four names Roblox's API dump gives them, which `luck.json` takes, and `JsonSchema` under the `schemars` feature.
- `definitions_for(variant, security)` returns the definitions a type checker loads over Luau's built-in ones for a script at that level, Roblox's, or `None` for standalone Luau.
- `StdlibDefinition::parse(text)` reads a std definition, and `Stdlib::new(variant).layer(&definition)` layers it over the built-in library.
- `analyze(chunk, counts, library)` returns a `SemanticAnalysis` of a tree, and beside it the semantic checker's errors. `SemanticAnalysis` answers `is_known_global`, `resolve_callee`, and `resolve_stdlib_path`, takes the configured extra globals with `add_extra_globals`, and holds the `Semantic`.

```rust
use luck_allocator::Allocator;
use luck_stdlib::{Stdlib, StdlibDefinition, Variant};

let definition = StdlibDefinition::parse(
    r#"
[globals.app]
kind = "namespace"

[globals.app.members.onStart]
kind = "function"
is_read_only = false
"#,
)
.expect("a valid std definition");
let stdlib = Stdlib::new(Variant::Standalone).layer(&definition).expect("it layers");

let allocator = Allocator::new();
let source = b"local greeting = 1 print(greeting, app)";
let parsed = luck_parser::parse(&allocator, source);
let chunk = allocator.alloc(parsed.chunk);
let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, stdlib.library());
assert!(errors.is_empty());
assert!(analysis.is_known_global("print"));
assert!(analysis.is_known_global("app"));
```

## Std definitions

A std definition is a TOML file. A project names its own in `luck.json`'s `std`, and they layer, in order, over the library of the project's variant.

```toml
[globals.app]
kind = "namespace"

[globals.app.members.net]
kind = "namespace"

# A function's params, required ones first, then optional ones, then at most one vararg.
[globals.app.members.net.members.connect]
kind = "function"
is_must_use = true
params = [{ kind = "string" }, { kind = "table", is_required = false }]
returns = "Socket"

# A callback the code defines, which is why it isn't read-only.
[globals.app.members.onMessage]
kind = "function"
is_read_only = false

[globals.app.members.net.members.send]
kind = "function"
overloads = [
  { params = [{ kind = "string" }, { kind = "string" }, { kind = "number", is_required = false }] },
  { params = [{ kind = "table" }] },
]

[shapes.Socket.members.isOpen]
kind = "function"
is_method = true
is_must_use = true

[constant_sets.protocols]
values = ["udp", { value = "tcp", is_common = true }]

[globals.app.members.net.members.listen]
kind = "function"
params = [{ kind = "constant", set = "protocols" }, { kind = "number" }]
```

- `[globals.<name>]` declares a global, and a namespace's `members` declare its fields, each with a `kind`: `function`, `constant`, `property`, or `namespace`. Every kind takes `deprecated = { message = "...", replacement = { template = "...", is_exact = true } }`, whose template names the old call's arguments `%1` to `%9`, and whose fix applies without asking only when it's exact.
- A `function` takes `params`, or `overloads`, a list of `{ params = [...] }`, and `is_pure`, `is_must_use`, `is_method` for one a colon calls on its shape, `is_read_only`, which is `true` unless set, and `returns`, the shape of its call's first result.
- A param takes a `kind`: `any`, `bool`, `number`, `string`, `function`, `table`, `display` with a `display` label, `constant` with its `values` or a `set`, or `vararg`. It's required unless `is_required = false`, except a `vararg`, which never is, and it can be `deprecated`. A value is a string, or a table with its `value`, `deprecated`, and `is_common`, which completion lists first.
- A `constant` or `property` takes `is_read_only` and `shape`. A `namespace` takes `members` and `shape`, for a namespace that's also a value with methods, and is always read-only.
- `[shapes.<name>]` declares the members of a kind of value, and `extends` names the shape whose members it has too, under its own.
- `[constant_sets.<name>]` declares the `values` a param's `set` names, here or in a definition or library below.
- `[enums.<Type>]` declares an `Enum`-shaped namespace of the `Enum` global, with its `items` as read-only `EnumItem`-shaped constants, as Roblox's are.
- Every function of the `string` namespace that takes a string first is also a method of the `string` shape, as a string's metatable makes it.

What a definition declares replaces what the library below declares under the same name, except that a namespace over a namespace, or a shape over a shape, keeps the members it doesn't declare, and the `shape`, `extends`, or deprecation it doesn't set. So a definition adds `string.trim` without repeating `string`, and a member it adds to Roblox's `Instance` reaches every shape that extends it.

A shape or a constant set a definition names must be declared by it, by a library below it, or by Roblox's built-in library. One that only Roblox's library declares is left unresolved on standalone Luau, so one definition serves both variants: the shape has no members, and a param that takes the set takes any string.
