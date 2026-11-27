# luck_type_checker

Rust bindings to Luau's type checker, built from Luau's C++ source.

This crate wraps the new type solver from Luau's Analysis library, compiled
from the Luau release it pins, with the flags `luau-analyze` turns on by
default. Errors and types come straight from Luau, in its own wording.

A `Checker` reads everything through a `Host` you implement, which supplies
each module's source, resolves `require` strings, and fills in each module's
Luau config, so nothing has to exist on disk. It keeps checked modules until
you `invalidate` them, checks several modules in parallel on threads it starts
and joins, and stops early when its `CancellationToken` is cancelled. After
`retain_type_graphs`, `describe` and `autocomplete` answer hover and completion
queries.

## Building

Building this crate compiles Luau's C++, which needs clang. Install LLVM, or on
macOS, Xcode's command line tools.

Luau's C++ includes its own VM and compiler, which run type functions. That
means this crate can't share a binary with another build of Luau, such as
`mlua`'s.

## Usage

This example checks a strict module that assigns a string to a `number` local:

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
assert_eq!(diags[0][0].code, "E015");
```

## API

* `Checker`, with `check`, `load_definitions`, `invalidate`,
  `retain_type_graphs`, `describe`, and `autocomplete`.
* `Checker::type_as_roblox_studio`, which adds the types Roblox Studio gives
  past what Roblox's definition file can express.
* `Host`, what you implement, with `source`, `resolve`, and `config`.
* `Config`, a module's Luau config, and `DirectoryConfigs`, which reads and
  keeps the `.luaurc` and `.config.luau` files of each directory.
* `CancellationToken`, which stops a running check from another thread.
* `CheckError`, why a check returned no diagnostics.
* `Description`, `Declaration`, and `Signature`, what `describe` finds.
* `Completion` and `CompletionKind`, what `autocomplete` finds.
* `builtin_definitions`, Luau's built-in definitions as text.
* `luau_config_aliases`, which runs a `.config.luau` in Luau's sandbox and
  returns its aliases.
