use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::mem;
use std::num::NonZero;
use std::thread;

use luck_ast::MIN_STACK_BYTES;
use luck_diagnostics::Diagnostic;
use luck_token::{LineIndex, Span, index_to_offset};
use rustc_hash::FxHashMap;

use crate::ffi::{
    CancellationToken, Config, LuauDescription, LuauDocs, LuauEntry, LuauError, LuckLocation,
    RawChecker, Task,
};

/// Where a checker reads everything from: modules by the names the host gives
/// them, what each `require` names, and each module's config. Nothing has to
/// be on disk.
///
/// A panic in a method aborts the process, since it can't unwind through
/// Luau's C++.
pub trait Host {
    /// The source of `module`, which the checker copies once, into Luau, so
    /// one the host holds can be borrowed, or `None` when no module has that
    /// name.
    fn source(&self, module: &str) -> Option<Cow<'_, [u8]>>;

    /// The module that `require(path)` in `requirer` names, or `None` when it
    /// names none.
    fn resolve(&self, requirer: &str, path: &str) -> Option<String>;

    /// Sets on `config` what the `.luaurc` and `.config.luau` files that
    /// govern `module` set, as [`DirectoryConfigs::apply`] does, or nothing.
    /// Analysis asks for each module's config twice a check.
    ///
    /// [`DirectoryConfigs::apply`]: crate::DirectoryConfigs::apply
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

/// What [`Checker::describe`] finds at an offset of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    /// The expression, local, or type name there.
    pub span: Span,
    /// Its type, as Luau's `toString` prints it, with the names of a
    /// function's parameters.
    pub ty: String,
    pub declaration: Declaration,
    /// Each overload of a function type, in order.
    pub signatures: Vec<Signature>,
}

/// What Analysis records of the declaration a name refers to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declaration {
    /// Luau's documentation symbol for it, which starts with the name the
    /// definitions that declare it were loaded under, as `@luau/global/print`
    /// or `@app/globaltype/Socket.Send`.
    pub documentation_symbol: Option<String>,
    /// The module or definitions it's declared in, by name, and the span of
    /// its declaration there.
    pub definition: Option<(String, Span)>,
}

/// A function's parameters and results, as Luau's `toStringNamedFunction`
/// prints them after a function's name, as `(s: string, n: number): string`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    /// Where each parameter is in `label`, without `self` for a method call.
    pub params: Vec<Span>,
}

/// An entry of Luau's autocomplete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub name: String,
    pub kind: CompletionKind,
    /// Its type, as [`Description::ty`] prints one.
    pub ty: Option<String>,
    pub is_deprecated: bool,
    /// What to insert instead of `name`, such as a function written out.
    pub insert_text: Option<String>,
    pub declaration: Declaration,
}

/// What a [`Completion`] is, as Luau's `AutocompleteEntryKind` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Property,
    Binding,
    Keyword,
    String,
    Type,
    Module,
    GeneratedFunction,
    RequirePath,
    HotComment,
}

pub(crate) struct Context<H> {
    pub(crate) host: H,
    /// Where each line starts in each module, as Luau last read it, and in
    /// each definition file loaded, by the name it was loaded under, and its
    /// length, which turn Luau's locations into spans.
    lines: RefCell<FxHashMap<String, (LineIndex, u32)>>,
    /// Each module whose config the host set as the check that asked was
    /// cancelled, which may have stopped it short of what its files set.
    /// Luau can still finish checking such a module, so the check marks it
    /// to be checked again.
    cut_short: RefCell<Vec<String>>,
}

impl<H: Host> Context<H> {
    pub(crate) fn source(&self, module: &str) -> Option<Cow<'_, [u8]>> {
        let source = self.host.source(module)?;
        let lines = (LineIndex::new(&source), index_to_offset(source.len()));
        self.lines.borrow_mut().insert(module.to_owned(), lines);
        Some(source)
    }

    pub(crate) fn config(&self, module: &str, config: &mut Config<'_>) {
        self.host.config(module, config);
        if config.is_cancelled() {
            self.cut_short.borrow_mut().push(module.to_owned());
        }
    }
}

impl<H> Context<H> {
    /// Where `location` is in the module or definitions named `name`.
    fn span(&self, name: &str, location: LuckLocation) -> Option<Span> {
        let lines = self.lines.borrow();
        span(location, lines.get(name)?)
    }

    fn declaration(&self, docs: LuauDocs) -> Declaration {
        let LuauDocs { symbol, definition } = docs;
        let definition = definition.and_then(|(name, location)| {
            let span = self.span(&name, location)?;
            Some((name, span))
        });
        Declaration { documentation_symbol: symbol, definition }
    }
}

impl<H: Host> Checker<H> {
    /// A checker with Luau's built-in definitions loaded.
    pub fn new(host: H) -> Self {
        let context = Context { host, lines: RefCell::default(), cut_short: RefCell::default() };
        Self { raw: RawChecker::new(context) }
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
            self.raw.context_mut().lines.get_mut().insert(name.to_owned(), lines);
            return Ok(());
        }
        Err(sorted(errors.into_iter().map(|error| diagnostic(error, Some(&lines))).collect()))
    }

    /// Keeps the types of each module checked after, which
    /// [`Self::describe`] and [`Self::autocomplete`] read, as Luau's
    /// `retainFullTypeGraphs` does, at the cost of the memory they take.
    pub fn retain_type_graphs(&mut self) {
        self.raw.retain_type_graphs();
    }

    /// The type reference, local, or expression at `offset` in `module`, as
    /// its last check left it, with its type, the declaration it refers to,
    /// and the signatures of a function type. A method call's signatures
    /// leave out `self`.
    ///
    /// It's `None` where there's none, and for a module checked before
    /// [`Self::retain_type_graphs`].
    pub fn describe(&mut self, module: &str, offset: u32) -> Option<Description> {
        let (line, column) = self.position(module, offset)?;
        let LuauDescription { location, ty, docs, signatures } =
            self.raw.describe(module, line, column)?;
        let context = self.raw.context();
        let span = context.span(module, location)?;
        Some(Description { span, ty, declaration: context.declaration(docs), signatures })
    }

    /// Luau's own autocomplete at `offset` in `module`, as its last check left
    /// it, without what the index there can't reach, such as a property after
    /// a colon, in no order, from the types [`Self::retain_type_graphs`]
    /// keeps.
    pub fn autocomplete(&mut self, module: &str, offset: u32) -> Vec<Completion> {
        let Some((line, column)) = self.position(module, offset) else {
            return Vec::new();
        };
        let entries = self.raw.autocomplete(module, line, column);
        let context = self.raw.context();
        entries
            .into_iter()
            .map(|entry| {
                let LuauEntry { name, kind, ty, is_deprecated, insert_text, docs } = entry;
                let declaration = context.declaration(docs);
                Completion { name, kind, ty, is_deprecated, insert_text, declaration }
            })
            .collect()
    }

    /// The zero-based line and byte column of `offset` in `module` as Luau
    /// last read it.
    fn position(&self, module: &str, offset: u32) -> Option<(u32, u32)> {
        let lines = self.raw.context().lines.borrow();
        let (index, length) = lines.get(module)?;
        let (line, column) = index.line_column(offset.min(*length));
        Some((line - 1, column - 1))
    }

    /// Types what Roblox Studio types past Roblox's definitions: the class a
    /// string literal names, for `Instance.new`, `GetService`, and
    /// `FindService`, or an unknown type when it names none that the member
    /// can return, and the `Enum.Material` its definitions call
    /// `EnumMaterial`.
    ///
    /// It applies to the modules checked after, as definitions do.
    pub fn type_as_roblox_studio(&mut self) {
        self.raw.return_named_type("Instance", "new");
        self.raw.return_named_type("ServiceProvider", "GetService");
        self.raw.return_named_type("ServiceProvider", "FindService");
        self.raw.bind_type_namespace("Enum", "EnumItem");
    }

    /// Marks `module`, and every module that requires it, to be checked
    /// again, such as after its source changes.
    pub fn invalidate(&mut self, module: &str) {
        self.raw.mark_dirty(module);
    }

    /// The diagnostics of each of `modules`, in order, from checking them and
    /// what they require, on a thread it starts and joins for each piece of
    /// work Luau hands out, up to `threads`, so a check with nothing to do
    /// starts none.
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
        let checked = thread::scope(|scope| {
            let (sender, receiver) = crossbeam_channel::unbounded::<Task>();
            let (mut starts, mut has_worker) = (0, false);
            let checked = self.raw.check(&names, cancellation, &mut |task| {
                if starts < threads.get() {
                    starts += 1;
                    let tasks = receiver.clone();
                    let worker = thread::Builder::new()
                        .name("luck-type-checker".to_owned())
                        .stack_size(MIN_STACK_BYTES)
                        .spawn_scoped(scope, move || tasks.iter().for_each(Task::run));
                    has_worker |= worker.is_ok();
                }
                if has_worker {
                    // `receiver` outlives the check, so the channel is open.
                    let _ = sender.send(task);
                } else {
                    task.run();
                }
            });
            drop(sender);
            checked
        });

        if cancellation.is_cancelled() {
            for module in mem::take(self.raw.context_mut().cut_short.get_mut()) {
                self.raw.mark_dirty(&module);
            }
            return Err(CheckError::Cancelled);
        }
        let lines = self.raw.context().lines.borrow();
        match checked {
            Err((module, error)) => {
                Err(CheckError::Internal { diag: diagnostic(error, lines.get(&module)), module })
            }
            Ok(errors) => Ok(errors
                .into_iter()
                .zip(&names)
                .map(|(errors, name)| {
                    let module_lines = lines.get(*name);
                    sorted(
                        errors.into_iter().map(|error| diagnostic(error, module_lines)).collect(),
                    )
                })
                .collect()),
        }
    }
}

impl<H> fmt::Debug for Checker<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Checker").finish_non_exhaustive()
    }
}

/// Where Luau's `location`, zero-based lines and byte columns, is in the
/// source whose lines and length are `lines`.
fn span(location: LuckLocation, lines: &(LineIndex, u32)) -> Option<Span> {
    let (lines, source_len) = lines;
    let LuckLocation { begin_line, begin_column, end_line, end_column } = location;
    let start = lines.offset(begin_line.checked_add(1)?, begin_column.saturating_add(1))?;
    let end = lines.offset(end_line.checked_add(1)?, end_column.saturating_add(1))?;
    let start = start.min(*source_len);
    Some(Span::new(start, end.clamp(start, *source_len)))
}

fn diagnostic(error: LuauError, lines: Option<&(LineIndex, u32)>) -> Diagnostic {
    let span = error.location.zip(lines).and_then(|(location, lines)| span(location, lines));
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
