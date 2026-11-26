# luck_type_checker

Rust bindings to Luau's official type checker, compiled from Luau's C++ source.

## Overview

It uses the new solver from Luau's Analysis library, built from the release the `luau` submodule pins, with the flags `luau-analyze` enables by default. Errors and types come straight from Luau.

The checker reads everything through a `Host` you implement, so nothing has to be on disk.

It keeps checked modules until you `invalidate` them, checks several modules in parallel, and stops early when its `CancellationToken` is cancelled.

After `retain_type_graphs`, `describe` and `autocomplete` answer hover and completion queries.

## Building

Building this crate compiles Luau's C++, which needs clang. Install LLVM, or Xcode's command line tools on macOS.

Luau's C++ includes its own VM and compiler, which run type functions. So this crate can't be linked into the same binary as another build of Luau, such as `mlua`'s.

## Usage

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

- `Checker` is the checker, with `check`, `load_definitions`, `invalidate`, `retain_type_graphs`, `describe`, and `autocomplete`.
- `Checker::type_as_roblox_studio` adds the types Roblox Studio gives past what Roblox's definition file can express.
- `Host` is what you implement, with `source`, `resolve`, and `config`.
- `Config` is a module's Luau config, and `DirectoryConfigs` sets it from the `.luaurc` and `.config.luau` files it keeps for each directory.
- `CancellationToken` stops a running check.
- `CheckError` says why a check returned no diagnostics.
- `Description`, `Declaration`, and `Signature` are what `describe` returns.
- `Completion` and `CompletionKind` are what `autocomplete` returns.
- `builtin_definitions` returns Luau's built-in definitions as text.
- `luau_config_aliases` runs a `.config.luau` in Luau's sandbox and returns its aliases.
