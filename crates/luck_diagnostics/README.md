# luck_diagnostics

The error and warning type every luck tool reports through, with codes, labels, and help text.

## Overview

A `Diagnostic` has a code, a severity, a message, and a span. It can also have labels pointing at other spans, help text, a note, a docs link, and an editor tag.

Each code has its own constructor, such as `Diagnostic::e008` for parse errors. Lint rules use `Diagnostic::lint`, with the rule name as the code.

A diagnostic doesn't know its file. `FileDiagnostics` pairs a list of them with a path.

## Usage

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

- `Diagnostic` boxes a `DiagnosticInner` and derefs to it.
- `DiagnosticInner` has the fields `code`, `severity`, `message`, `span`, `labels`, `help`, `note`, `url`, and `tag`.
- `Severity` is `Error` or `Warning`.
- `Label` is a secondary span with a message.
- `Tag` is `Unnecessary` or `Deprecated`, which editors show as faded or struck through.
- `FileDiagnostics` is a path and its diagnostics.
