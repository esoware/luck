//! # `luck_diagnostics`
//!
//! The one diagnostic type every luck tool reports a problem with, modeled
//! on oxc's `OxcDiagnostic`.
//!
//! A [`Diagnostic`] holds a code, a [`Severity`], a message, the span where
//! the problem is, [`Label`]s for the other places it's about, help, a note,
//! a link to the page that documents it, and a [`Tag`] for an editor. It
//! names no file, so a list of them travels with its file in a
//! [`FileDiagnostics`].
//!
//! Every coded one is built by a numbered constructor, such as
//! [`Diagnostic::e008`] for a lex or parse failure, so each code is defined
//! in one place, and a lint rule's by [`Diagnostic::lint`], with the rule's
//! name as its code.
//!
//! # Usage
//!
//! ```
//! use luck_diagnostics::Diagnostic;
//! use luck_token::Span;
//!
//! let message = "expected `end` to close `function`, found end of file";
//! let diag = Diagnostic::e008(Span::new(18, 18), message)
//!     .with_label(Span::new(6, 14), "unclosed `function`");
//! assert_eq!(diag.code, "E008");
//! assert_eq!(diag.labels[0].message, "unclosed `function`");
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod codes;
mod diagnostic;
mod file_diagnostics;

pub use diagnostic::{Diagnostic, DiagnosticInner, Label, Severity, Tag};
pub use file_diagnostics::FileDiagnostics;
