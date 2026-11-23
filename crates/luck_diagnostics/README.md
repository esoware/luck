# luck_diagnostics

The one diagnostic type every luck tool reports a problem with, modeled on oxc's `OxcDiagnostic`.

## Usage

- `Diagnostic`: a code, a `Severity`, a message, the span where the problem is, `Label`s for the other places it's about, such as the construct a missing `end` leaves open, help, a note, a link to the page that documents it, and a `Tag` that tells an editor the code it points at is unnecessary or deprecated. Its fields sit behind one box, so a `Result` or a list of them holds a pointer each, and it derefs to them.
- Numbered constructors build every coded one, such as `Diagnostic::e008` for a lex or parse failure and `Diagnostic::e004` for a module that isn't found, so each code is defined in one place. `Diagnostic::lint` builds a lint rule's, with the rule's name as its code.
- `FileDiagnostics`: a file's path and the diagnostics in it. A diagnostic names no file, so the path is held once per file, not by each diagnostic.

The CLI renders diagnostics in its output formats, and the language server sends them to the editor, with each label as a related location.

```rust
use luck_diagnostics::Diagnostic;
use luck_token::Span;

let message = "expected `end` to close `function`, found end of file";
let diag = Diagnostic::e008(Span::new(18, 18), message)
    .with_label(Span::new(6, 14), "unclosed `function`");
assert_eq!(diag.code, "E008");
assert_eq!(diag.labels[0].message, "unclosed `function`");
```
