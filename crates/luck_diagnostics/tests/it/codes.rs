use std::io;

use luck_diagnostics::{Diagnostic, Severity};
use luck_token::Span;

#[test]
fn constructors_set_code_severity_and_help() {
    let e2 = Diagnostic::e002(Span::new(0, 10));
    assert_eq!((e2.code, e2.span), ("E002", Some(Span::new(0, 10))));
    assert!(e2.is_error());

    let e4 = Diagnostic::e004(Span::new(0, 10), "mymod", &["src/mymod.lua".to_owned()]);
    assert_eq!(
        e4.help.as_deref(),
        Some(
            "\
searched paths:
  - `src/mymod.lua`"
        )
    );

    let w1 = Diagnostic::w001(Span::new(20, 25), "utils", Span::new(0, 5));
    assert_eq!(w1.code, "W001");
    assert!(!w1.is_error());
    assert!(w1.help.is_none());
    assert_eq!(w1.labels[0].span, Span::new(0, 5));
}

#[test]
fn a_problem_with_no_place_has_no_span() {
    let io_error = io::Error::from(io::ErrorKind::NotFound);
    assert_eq!(Diagnostic::e010("gone.lua", &io_error).span, None);
    assert_eq!(Diagnostic::e014(None, "bad").span, None);
    let cycle = Diagnostic::w003(None, &["a.lua".to_owned(), "b.lua".to_owned()]);
    assert_eq!((cycle.severity, cycle.span), (Severity::Warning, None));
}

/// A help that wraps in the source reads as one line, but where it breaks one
/// on purpose.
#[test]
fn a_wrapped_help_joins_its_lines_with_single_spaces() {
    let cycle = Diagnostic::w003(None, &["a.luau".to_owned(), "b.luau".to_owned()]);
    assert_eq!(
        cycle.help.as_deref(),
        Some(
            "\
cycle: `a.luau` -> `b.luau`
modules load lazily, so a cycle through a function body works, but requiring a module while it's still loading raises an error at runtime"
        )
    );
    let hot_comment = Diagnostic::w006(Span::new(0, 9), "--!strict");
    assert_eq!(
        hot_comment.help.as_deref(),
        Some(
            "Luau hot comments only apply at the top of a chunk, so only the entry module's are hoisted above the bundle's loader"
        )
    );
}
