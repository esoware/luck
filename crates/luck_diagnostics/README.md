# luck_diagnostics

A diagnostic type for reporting errors and warnings in source code.

A `Diagnostic` has a code, a severity, a message, and the span the problem is
at. It can also carry labels that point at related spans, a help line, a note,
a link to the code's documentation, and a tag that tells an editor to show the
code as unnecessary or deprecated.

Every code has exactly one constructor, such as `Diagnostic::e008` for a syntax
error, so each code's meaning is defined in one place. A lint diagnostic is
built with `Diagnostic::lint` instead, with the rule's name as its code.

A diagnostic doesn't record its file. Diagnostics that travel between files go
in a `FileDiagnostics`, which pairs them with a path.

## Usage

This example reports an unclosed function, with a label on the `function`
keyword:

```rust
use luck_diagnostics::Diagnostic;
use luck_token::Span;

let message = "expected `end` to close `function`, found end of file";
let diag = Diagnostic::e008(Span::new(18, 18), message)
    .with_label(Span::new(6, 14), "unclosed `function`");
assert_eq!(diag.code, "E008");
assert_eq!(diag.labels[0].message, "unclosed `function`");
```

## API

* `Diagnostic`, which boxes a `DiagnosticInner` holding its fields, and derefs
  to it.
* `Severity`, either `Error` or `Warning`.
* `Label`, another span the diagnostic is about, with its own message.
* `Tag`, either `Unnecessary` or `Deprecated`.
* `FileDiagnostics`, a path and the diagnostics in that file.
