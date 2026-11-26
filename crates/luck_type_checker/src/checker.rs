use std::cell::RefCell;
use std::fmt;
use std::num::NonZero;
use std::thread;

use crossbeam_channel::SendError;
use luck_ast::MIN_STACK_BYTES;
use luck_diagnostics::Diagnostic;
use luck_token::{LineIndex, Span, index_to_offset};
use rustc_hash::FxHashMap;

use crate::ffi::{CancellationToken, Config, LuauError, Outcome, RawChecker, Task};

/// Where a checker reads everything from: modules by the names the host gives
/// them, what each `require` names, and each module's config. Nothing has to
/// be on disk.
///
/// A panic in a method aborts the process, since it can't unwind through
/// Luau's C++.
pub trait Host {
    /// The source of `module`, or `None` when no module has that name.
    fn source(&self, module: &str) -> Option<Vec<u8>>;

    /// The module that `require(path)` in `requirer` names, or `None` when it
    /// names none.
    fn resolve(&self, requirer: &str, path: &str) -> Option<String>;

    /// Applies the `.luaurc` files that govern `module` to `config`,
    /// outermost first.
    fn config(&self, module: &str, config: &mut Config<'_>);
}

/// Luau's own type checker, the new solver of its Analysis library, over a
/// [`Host`].
///
/// It keeps every module it checks, and checks a module again only once
/// [`Self::invalidate`] marks it or a module it requires.
pub struct Checker<H> {
    raw: RawChecker<H>,
}

/// Why a check returned no diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// Its [`CancellationToken`] was cancelled.
    Cancelled,
    /// Luau threw an internal compiler error, a bug in Luau, checking
    /// `module`.
    Internal { module: String, diag: Diagnostic },
}

pub(crate) struct Context<H> {
    pub(crate) host: H,
    /// Where each line starts in each module, as Luau last read it, and its
    /// length, which turn Luau's locations into spans.
    lines: RefCell<FxHashMap<String, (LineIndex, u32)>>,
}

impl<H: Host> Context<H> {
    pub(crate) fn source(&self, module: &str) -> Option<Vec<u8>> {
        let source = self.host.source(module)?;
        let lines = (LineIndex::new(&source), index_to_offset(source.len()));
        self.lines.borrow_mut().insert(module.to_owned(), lines);
        Some(source)
    }
}

impl<H: Host> Checker<H> {
    /// A checker with Luau's built-in definitions loaded.
    pub fn new(host: H) -> Self {
        Self { raw: RawChecker::new(Context { host, lines: RefCell::default() }) }
    }

    /// The host, to change what it holds. Invalidate a module it changes.
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.raw.context_mut().host
    }

    /// Loads the definitions in `source`, which Luau names `name`, as its
    /// `loadDefinitionFile` does, for the modules checked after.
    ///
    /// # Errors
    ///
    /// Returns what's wrong with `source`, with spans in it, and loads none of
    /// it.
    pub fn load_definitions(&mut self, name: &str, source: &str) -> Result<(), Vec<Diagnostic>> {
        let lines = (LineIndex::new(source.as_bytes()), index_to_offset(source.len()));
        let errors = self.raw.load_definitions(name, source);
        if errors.is_empty() {
            return Ok(());
        }
        Err(sorted(errors.into_iter().map(|error| diagnostic(error, Some(&lines))).collect()))
    }

    /// Marks `module`, and every module that requires it, to be checked
    /// again, such as after its source changes.
    pub fn invalidate(&mut self, module: &str) {
        self.raw.mark_dirty(module);
    }

    /// The diagnostics of each of `modules`, in order, from checking them and
    /// what they require on `threads` threads it starts and joins.
    ///
    /// Each is an E015, or an E008 for a syntax error, in Luau's own words,
    /// and a module no source was found for has none. Luau decides each
    /// module's mode, from its hot comment or else its config.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::Cancelled`] once `cancellation` is cancelled, or
    /// the internal compiler error Luau threw.
    pub fn check<Module: AsRef<str>>(
        &mut self,
        modules: &[Module],
        threads: NonZero<usize>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<Vec<Diagnostic>>, CheckError> {
        let names: Vec<&str> = modules.iter().map(AsRef::as_ref).collect();
        let (sender, receiver) = crossbeam_channel::unbounded::<Task>();
        let outcome = thread::scope(|scope| {
            for _ in 0..threads.get() {
                let tasks = receiver.clone();
                // A worker that can't start leaves its tasks to the others, or
                // to this thread once there are none.
                let _ = thread::Builder::new()
                    .name("luck-type-checker".to_owned())
                    .stack_size(MIN_STACK_BYTES)
                    .spawn_scoped(scope, move || tasks.iter().for_each(Task::run));
            }
            drop(receiver);
            let checked = self.raw.check(&names, cancellation, &mut |task| {
                if let Err(SendError(task)) = sender.send(task) {
                    task.run();
                }
            });
            drop(sender);
            checked
        });

        let lines = self.raw.context().lines.borrow();
        match outcome {
            Outcome::Cancelled => Err(CheckError::Cancelled),
            Outcome::InternalError(error) => {
                let module = error.module_name.clone();
                Err(CheckError::Internal { diag: diagnostic(error, lines.get(&module)), module })
            }
            Outcome::Checked(errors) => {
                let mut diags = vec![Vec::new(); names.len()];
                for error in errors {
                    let module_lines = names.get(error.module).and_then(|name| lines.get(*name));
                    if let Some(module_diags) = diags.get_mut(error.module) {
                        module_diags.push(diagnostic(error, module_lines));
                    }
                }
                Ok(diags.into_iter().map(sorted).collect())
            }
        }
    }
}

impl<H> fmt::Debug for Checker<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Checker").finish_non_exhaustive()
    }
}

fn diagnostic(error: LuauError, lines: Option<&(LineIndex, u32)>) -> Diagnostic {
    let span = error.location.zip(lines).and_then(|(location, (lines, source_len))| {
        let [begin_line, begin_column, end_line, end_column] = location;
        let start = lines.offset(begin_line.checked_add(1)?, begin_column.saturating_add(1))?;
        let end = lines.offset(end_line.checked_add(1)?, end_column.saturating_add(1))?;
        let start = start.min(*source_len);
        Some(Span::new(start, end.clamp(start, *source_len)))
    });
    if error.is_syntax_error {
        Diagnostic::e008(span.unwrap_or_default(), error.message)
    } else {
        Diagnostic::e015(span, error.message)
    }
}

/// In source order, as luck reports a file's diagnostics.
fn sorted(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags.sort_by_key(|diag| diag.span.map(|span| span.start));
    diags
}
