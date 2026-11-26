# luck_type_checker

Luau's own type checker for the luck toolchain: the new solver of the Analysis library in [luau-lang/luau](https://github.com/luau-lang/luau), built from source, not rewritten.

## Overview

The `luau` directory is a git submodule pinned to a Luau release, and `just luau <tag>` moves it to another. `build.rs` compiles Analysis and what it links, Ast, Common, Config, and the Compiler, Bytecode, and VM that run type functions, with a small C++ shim that gives them a C ABI. It compiles them with clang, the one `CXX` names, or else `clang-cl` for an MSVC target and `clang++` for any other, from `PATH` or, on Windows, from `C:\Program Files\LLVM\bin`, and the build fails at once without one. It compiles at `-O3` with `NDEBUG`, which turns Luau's assertions off, and the VM with `-fno-math-errno` too, as Luau's own build does. When rustc's `-Clinker-plugin-lto` leaves LTO to the linker, as `just release-binary` asks, it compiles them to LLVM bitcode with `-flto`, so the linker optimizes the C++ with the Rust. The `ffi` module declares that ABI and holds every `unsafe` in the crate. No C++ exception reaches Rust: the shim catches the internal compiler errors Analysis throws and reports them, and any other exception, such as running out of memory, ends the process, as it does in Rust. Luau's flags are process-wide, so the first checker sets them once, to the ones `luau-analyze` turns on by default.

A `Checker` reads everything through a `Host`: a module's source by the name the host gives it, which the host can lend, since the checker copies it once, into Luau, the module a `require` string names from a module, and the `.luaurc` and `.config.luau` files that govern a module, which Luau's own Config library reads into its config, parsing a `.luaurc` and running a `.config.luau` in its sandboxed VM, with no `require`, returning its error for one it can't read. So a checker works with nothing on disk, such as over an editor's buffers. `luau_config_aliases` runs a `.config.luau` the same way for a caller that wants only its aliases, as `luck_resolver` does, which links no Luau of its own. As in `luau-analyze`, the VM's interrupt throws Luau's `TimeLimitError` once a `.config.luau` has run for a second, and in a check, its `UserCancelError` once the check's `CancellationToken` is cancelled, which the shim reports as the config's error. A real config returns its table in tens of microseconds, and one with thousands of aliases in a few milliseconds, so a second leaves room for a machine too loaded to run one at once, where a config stopped short would lose its mode and aliases, and still stops one that never returns. Unlike `luau-analyze`, the shim keeps the limits in the VM's callbacks, not in a thread's data, which a coroutine doesn't inherit. Analysis asks the host only on the thread that called into the checker. A module's mode is Luau's to decide, from its hot comment or else its config, nonstrict by default. A `require` the host resolves to nothing is unknown, which Luau reports in strict mode only.

`check` hands the modules to Analysis's own queue, `checkQueuedModules`, which splits the work into tasks that worker threads the checker starts run, each with `luck_ast::MIN_STACK_BYTES` of stack, and joins before it returns. A checker keeps every module it checks, so the next check reads only what `invalidate` marked changed and the modules that require it. A `CancellationToken` stops a check from any thread. Asked to, a checker keeps each module's types too, as Luau's `retainFullTypeGraphs` does, which an editor reads through Analysis's own queries: the type at a position, printed by Luau's `toString`, and the declaration it refers to, by its documentation symbol and where Analysis records it, through `findExprOrLocalAtPosition` and `getDocumentationSymbolAtPosition`, and Luau's `autocomplete`.

Each error is a `luck_diagnostics::Diagnostic` with Luau's own message, an E015, or an E008 for a syntax error, at a span in the module's bytes, and a module's diagnostics are in source order.

Luau's C++ includes its VM and compiler, so a binary can't link this crate and another build of Luau, such as `mlua`'s.

## Usage

- `Checker::new(host)` starts from Luau's built-in definitions, and `builtin_definitions()` gives their text.
- `load_definitions(name, source)` loads a definition file from text, as Luau's `loadDefinitionFile` does, or returns its errors and loads none of it.
- `Config::apply_luaurc(text)` and `Config::apply_luau_config(source)` apply a `.luaurc` or a `.config.luau` over a module's config, in `Host::config`, the latter stopped once it has run for a second or the check is cancelled, and `luau_config_aliases(source)` gives the aliases a `.config.luau` declares, by lowercased name, or Luau's error, stopped once it has run for a second.
- `return_named_type(owner, member)` and `bind_type_namespace(namespace, supertype)` type what a definition file can't, through Analysis's own magic functions and scopes, for a host such as Roblox Studio that gives them: a call of `member` of the global table or extern type `owner` with a string literal returns the extern type it names, when that extends the type `member` returns, and reports any other name as an unknown type, and each extern type that extends `supertype` and whose name is `namespace` and more is a type of the namespace, by the rest of its name. A checker applies neither unless asked.
- `check(modules, threads, cancellation)` returns each module's diagnostics in order, or `CheckError::Cancelled`, or the internal compiler error Luau threw.
- `invalidate(module)` marks a module and every module that requires it to be checked again, and `host_mut()` reaches the host to change what it holds.
- `retain_type_graphs()` keeps the types of each module checked after, which `describe` and `autocomplete` read.
- `describe(module, offset)` returns the type reference, local, or expression at a byte offset as the last check left it, as a `Description`: its span, its type as Luau's `toString` prints it, with parameter names, its `Declaration`, and each overload of a function type as a `Signature`, Luau's `toStringNamedFunction` without the name, with where each parameter is in it, `self` left out for a method call.
- `autocomplete(module, offset)` returns Luau's own autocomplete entries as `Completion`s, each with its `CompletionKind`, type, deprecation, the text to insert when it differs, and its `Declaration`, leaving out what the index there can't reach.
- A `Declaration` is the documentation symbol Luau gives what a name refers to, as `@luau/global/print` or `@app/globaltype/Socket.Send`, which starts with the name its definitions were loaded under, and the module or definitions, by name, and span of its declaration, where Analysis records one, as it does for a property.
- `CancellationToken::is_cancelled()` says whether a token was cancelled.

```rust
use std::borrow::Cow;
use std::num::NonZero;

use luck_type_checker::{CancellationToken, Checker, Config, Host};

struct OneModule;

impl Host for OneModule {
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        let source = b"--!strict\nlocal x: number = \"one\"\n";
        (module == "main").then_some(Cow::Borrowed(source.as_slice()))
    }

    fn resolve(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    fn config(&self, _: &str, _: &mut Config<'_>) {}
}

let mut checker = Checker::new(OneModule);
let diags = checker.check(&["main"], NonZero::<usize>::MIN, &CancellationToken::new()).unwrap();
assert_eq!(diags[0][0].message, "Expected this to be 'number', but got 'string'");
```

## License

Luau is under the MIT license, in `luau/LICENSE.txt`, and its VM keeps Lua's, in `luau/lua_LICENSE.txt`.
