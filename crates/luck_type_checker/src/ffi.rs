//! The C ABI of `shim.cpp`, and the safe types over it.
//!
//! Analysis calls a host's callbacks only on the thread that called into the
//! checker, since `Frontend::checkQueuedModules` reads, parses, and resolves
//! the whole module graph before it hands out a task, and a task asks the
//! host nothing. So a context is read on one thread at a time. A Rust panic
//! in a callback aborts, since an `extern "C"` function can't unwind.
//!
//! What the shim passes a callback, a string or a record, lives until the
//! callback returns, and a null string is none. Each `Cxx` type is one of the
//! shim's C++ objects, which Rust only points at.

#![expect(
    unsafe_code,
    reason = "the type checker is Luau's C++, which only a foreign call reaches"
)]

use std::borrow::Cow;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::{iter, slice, str};

use luck_token::Span;

use crate::checker::{CompletionKind, Context, Host, Signature};

#[repr(C)]
struct CxxChecker([u8; 0]);

#[repr(C)]
struct CxxString([u8; 0]);

#[repr(C)]
struct CxxConfig([u8; 0]);

#[repr(C)]
struct CxxTask([u8; 0]);

#[repr(C)]
struct CxxCancellationToken([u8; 0]);

#[repr(C)]
#[derive(Clone, Copy)]
struct LuckStr {
    data: *const u8,
    len: usize,
}

impl LuckStr {
    const fn new(bytes: &[u8]) -> Self {
        Self { data: bytes.as_ptr(), len: bytes.len() }
    }

    /// Its bytes, or `None` for a null string.
    ///
    /// # Safety
    ///
    /// `data` is null, or points at `len` bytes that stay put and unchanged
    /// for `'bytes`.
    unsafe fn bytes<'bytes>(self) -> Option<&'bytes [u8]> {
        if self.data.is_null() {
            return None;
        }
        // SAFETY: the caller promises `len` readable bytes at `data`.
        Some(unsafe { slice::from_raw_parts(self.data, self.len) })
    }

    /// # Safety
    ///
    /// As for [`Self::bytes`], for this call.
    unsafe fn optional(self) -> Option<String> {
        // SAFETY: the caller promises it.
        unsafe { self.bytes() }.map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    /// # Safety
    ///
    /// As for [`Self::bytes`], for this call.
    unsafe fn owned(self) -> String {
        // SAFETY: the caller promises it.
        unsafe { self.optional() }.unwrap_or_default()
    }
}

/// Where something is as Luau counts it: lines and byte columns from 0.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LuckLocation {
    pub(crate) begin_line: u32,
    pub(crate) begin_column: u32,
    pub(crate) end_line: u32,
    pub(crate) end_column: u32,
}

#[repr(C)]
struct LuckHost {
    context: *mut c_void,
    source:
        extern "C" fn(context: *mut c_void, module: LuckStr, source: NonNull<CxxString>) -> bool,
    resolve: extern "C" fn(
        context: *mut c_void,
        requirer: LuckStr,
        path: LuckStr,
        module: NonNull<CxxString>,
    ) -> bool,
    config: extern "C" fn(
        context: *mut c_void,
        module: LuckStr,
        config: NonNull<CxxConfig>,
        cancellation: NonNull<CxxCancellationToken>,
    ),
}

#[repr(C)]
struct LuckError {
    module: LuckStr,
    is_syntax_error: bool,
    has_location: bool,
    location: LuckLocation,
    message: LuckStr,
}

#[repr(C)]
struct LuckErrors {
    context: *mut c_void,
    push: extern "C" fn(context: *mut c_void, error: &LuckError),
}

impl LuckErrors {
    fn new(errors: &mut Vec<LuauError>) -> Self {
        Self { context: ptr::from_mut(errors).cast(), push: push_error }
    }
}

#[repr(C)]
struct LuckModule {
    name: LuckStr,
    errors: LuckErrors,
}

#[repr(C)]
struct LuckExecutor {
    context: *mut c_void,
    submit: extern "C" fn(context: *mut c_void, task: NonNull<CxxTask>),
}

#[repr(C)]
struct LuckDocs {
    symbol: LuckStr,
    module: LuckStr,
    location: LuckLocation,
}

impl LuckDocs {
    /// # Safety
    ///
    /// Its strings are the shim's for the callback it's passed to.
    unsafe fn owned(&self) -> LuauDocs {
        // SAFETY: the caller promises it.
        let symbol = unsafe { self.symbol.optional() };
        // SAFETY: as for the symbol.
        let module = unsafe { self.module.optional() };
        LuauDocs { symbol, definition: module.map(|module| (module, self.location)) }
    }
}

#[repr(C)]
struct LuckRange {
    start: u32,
    end: u32,
}

#[repr(C)]
struct LuckDescribed {
    context: *mut c_void,
    describe:
        extern "C" fn(context: *mut c_void, location: LuckLocation, ty: LuckStr, docs: &LuckDocs),
    signature:
        extern "C" fn(context: *mut c_void, label: LuckStr, params: *const LuckRange, count: usize),
}

#[repr(C)]
struct LuckEntry {
    name: LuckStr,
    /// Luau's `AutocompleteEntryKind`.
    kind: u8,
    ty: LuckStr,
    is_deprecated: bool,
    insert_text: LuckStr,
    docs: LuckDocs,
}

#[repr(C)]
struct LuckEntries {
    context: *mut c_void,
    push: extern "C" fn(context: *mut c_void, entry: &LuckEntry),
}

#[repr(C)]
struct LuckAliases {
    context: *mut c_void,
    push: extern "C" fn(context: *mut c_void, name: LuckStr, value: LuckStr),
}

unsafe extern "C" {
    fn luck_string_assign(string: NonNull<CxxString>, value: LuckStr);
    fn luck_config_apply_luaurc(config: NonNull<CxxConfig>, text: LuckStr, errors: LuckErrors);
    fn luck_config_apply_luau_config(
        config: NonNull<CxxConfig>,
        cancellation: NonNull<CxxCancellationToken>,
        source: LuckStr,
        errors: LuckErrors,
    );
    fn luck_luau_config_aliases(source: LuckStr, errors: LuckErrors, aliases: LuckAliases);
    fn luck_config_save(config: NonNull<CxxConfig>) -> NonNull<CxxConfig>;
    fn luck_config_set(config: NonNull<CxxConfig>, saved: NonNull<CxxConfig>);
    fn luck_config_free(config: NonNull<CxxConfig>);
    fn luck_checker_new(host: LuckHost) -> NonNull<CxxChecker>;
    fn luck_checker_free(checker: NonNull<CxxChecker>);
    fn luck_checker_load_definitions(
        checker: NonNull<CxxChecker>,
        name: LuckStr,
        source: LuckStr,
        errors: LuckErrors,
    );
    fn luck_checker_return_named_type(
        checker: NonNull<CxxChecker>,
        owner: LuckStr,
        member: LuckStr,
    );
    fn luck_checker_bind_type_namespace(
        checker: NonNull<CxxChecker>,
        type_namespace: LuckStr,
        supertype: LuckStr,
    );
    fn luck_checker_retain_type_graphs(checker: NonNull<CxxChecker>);
    fn luck_checker_mark_dirty(checker: NonNull<CxxChecker>, module: LuckStr);
    fn luck_checker_check(
        checker: NonNull<CxxChecker>,
        modules: *const LuckModule,
        count: usize,
        cancellation: NonNull<CxxCancellationToken>,
        executor: LuckExecutor,
        errors: LuckErrors,
    );
    fn luck_checker_describe(
        checker: NonNull<CxxChecker>,
        module: LuckStr,
        line: u32,
        column: u32,
        described: LuckDescribed,
    );
    fn luck_checker_autocomplete(
        checker: NonNull<CxxChecker>,
        module: LuckStr,
        line: u32,
        column: u32,
        entries: LuckEntries,
    );
    fn luck_task_run(task: NonNull<CxxTask>);
    safe fn luck_cancellation_new() -> NonNull<CxxCancellationToken>;
    fn luck_cancellation_cancel(token: NonNull<CxxCancellationToken>);
    fn luck_cancellation_is_cancelled(token: NonNull<CxxCancellationToken>) -> bool;
    fn luck_cancellation_free(token: NonNull<CxxCancellationToken>);
    safe fn luck_builtin_definitions() -> LuckStr;
}

/// An error Analysis reported.
pub(crate) struct LuauError {
    pub(crate) is_syntax_error: bool,
    pub(crate) location: Option<LuckLocation>,
    pub(crate) message: String,
}

impl LuauError {
    /// # Safety
    ///
    /// Its strings are the shim's for the callback it's passed to.
    unsafe fn new(error: &LuckError) -> Self {
        Self {
            is_syntax_error: error.is_syntax_error,
            location: error.has_location.then_some(error.location),
            // SAFETY: the caller promises it.
            message: unsafe { error.message.owned() },
        }
    }
}

/// What Analysis records of the declaration a name refers to: Luau's
/// documentation symbol for it, and the module and location it's declared at.
pub(crate) struct LuauDocs {
    pub(crate) symbol: Option<String>,
    pub(crate) definition: Option<(String, LuckLocation)>,
}

pub(crate) struct LuauDescription {
    pub(crate) location: LuckLocation,
    pub(crate) ty: String,
    pub(crate) docs: LuauDocs,
    pub(crate) signatures: Vec<Signature>,
}

pub(crate) struct LuauEntry {
    pub(crate) name: String,
    pub(crate) kind: CompletionKind,
    pub(crate) ty: Option<String>,
    pub(crate) is_deprecated: bool,
    pub(crate) insert_text: Option<String>,
    pub(crate) docs: LuauDocs,
}

/// A frontend over the host in a [`Context`] it owns.
pub(crate) struct RawChecker<H> {
    checker: NonNull<CxxChecker>,
    context: NonNull<Context<H>>,
}

// SAFETY: a frontend keeps nothing tied to the thread that made it, and the
// host moves with it.
unsafe impl<H: Send> Send for RawChecker<H> {}

impl<H: Host> RawChecker<H> {
    pub(crate) fn new(context: Context<H>) -> Self {
        let context = NonNull::from(Box::leak(Box::new(context)));
        let host = LuckHost {
            context: context.as_ptr().cast(),
            source: read_source::<H>,
            resolve: resolve::<H>,
            config: read_config::<H>,
        };
        // SAFETY: the context outlives the frontend, which `drop` frees first.
        let checker = unsafe { luck_checker_new(host) };
        Self { checker, context }
    }

    pub(crate) const fn context(&self) -> &Context<H> {
        // SAFETY: the context lives as long as `self`, and is only read while
        // no call into the frontend is running, which takes `&mut self`.
        unsafe { self.context.as_ref() }
    }

    pub(crate) fn context_mut(&mut self) -> &mut Context<H> {
        // SAFETY: as in `context`, and `&mut self` excludes every other borrow.
        unsafe { self.context.as_mut() }
    }

    pub(crate) fn load_definitions(&mut self, name: &str, source: &str) -> Vec<LuauError> {
        let mut errors = Vec::new();
        // SAFETY: the strings and `errors` outlive the call, which copies the
        // strings.
        unsafe {
            luck_checker_load_definitions(
                self.checker,
                LuckStr::new(name.as_bytes()),
                LuckStr::new(source.as_bytes()),
                LuckErrors::new(&mut errors),
            );
        }
        errors
    }

    /// Makes a call of `member`, a function in the table the global `owner`
    /// holds, or else a method of the extern type `owner`, whose first
    /// argument is a string literal, return the global extern type the string
    /// names when that extends the extern type `member` returns, or else
    /// report the name as an unknown type. It does nothing when `owner` has no
    /// such member or the member returns no extern type.
    pub(crate) fn return_named_type(&mut self, owner: &str, member: &str) {
        // SAFETY: the names outlive the call, which copies them.
        unsafe {
            luck_checker_return_named_type(
                self.checker,
                LuckStr::new(owner.as_bytes()),
                LuckStr::new(member.as_bytes()),
            );
        }
    }

    /// Moves each extern type that extends `supertype`, and whose name is
    /// `namespace` and more, under the type namespace `namespace` by the rest
    /// of its name, which Luau then names it after, so `EnumMaterial` is
    /// `Enum.Material`.
    pub(crate) fn bind_type_namespace(&mut self, namespace: &str, supertype: &str) {
        // SAFETY: the names outlive the call, which copies them.
        unsafe {
            luck_checker_bind_type_namespace(
                self.checker,
                LuckStr::new(namespace.as_bytes()),
                LuckStr::new(supertype.as_bytes()),
            );
        }
    }

    pub(crate) fn retain_type_graphs(&mut self) {
        // SAFETY: the frontend lives as long as `self`.
        unsafe { luck_checker_retain_type_graphs(self.checker) };
    }

    pub(crate) fn mark_dirty(&mut self, module: &str) {
        // SAFETY: the name outlives the call, which copies it.
        unsafe { luck_checker_mark_dirty(self.checker, LuckStr::new(module.as_bytes())) };
    }

    /// The errors of each of `modules`, in order, from checking them, or the
    /// internal compiler error Analysis threw, with the module it threw in. It
    /// hands each task Analysis splits the work into to `submit`, and returns
    /// once every task has run.
    pub(crate) fn check(
        &mut self,
        modules: &[&str],
        cancellation: &CancellationToken,
        mut submit: &mut dyn FnMut(Task),
    ) -> Result<Vec<Vec<LuauError>>, (String, LuauError)> {
        let mut errors: Vec<Vec<LuauError>> =
            iter::repeat_with(Vec::new).take(modules.len()).collect();
        let checked: Vec<LuckModule> = modules
            .iter()
            .zip(&mut errors)
            .map(|(module, module_errors)| LuckModule {
                name: LuckStr::new(module.as_bytes()),
                errors: LuckErrors::new(module_errors),
            })
            .collect();
        let mut failure: Option<(String, LuauError)> = None;
        let executor = LuckExecutor { context: (&raw mut submit).cast(), submit: submit_task };
        let failures = LuckErrors { context: (&raw mut failure).cast(), push: push_failure };
        // SAFETY: the names, the token, `submit`, every list of errors, and
        // `failure` outlive the call, which waits for every task it submits.
        unsafe {
            luck_checker_check(
                self.checker,
                checked.as_ptr(),
                checked.len(),
                cancellation.raw.0,
                executor,
                failures,
            );
        }
        failure.map_or(Ok(errors), Err)
    }

    /// What's at the zero-based `line` and byte `column` of `module`.
    pub(crate) fn describe(
        &mut self,
        module: &str,
        line: u32,
        column: u32,
    ) -> Option<LuauDescription> {
        let mut described: Option<LuauDescription> = None;
        let sink = LuckDescribed {
            context: (&raw mut described).cast(),
            describe: push_description,
            signature: push_signature,
        };
        // SAFETY: the name and `described` outlive the call, which copies the
        // name.
        unsafe {
            luck_checker_describe(
                self.checker,
                LuckStr::new(module.as_bytes()),
                line,
                column,
                sink,
            );
        }
        described
    }

    /// Luau's autocomplete at the zero-based `line` and byte `column` of
    /// `module`.
    pub(crate) fn autocomplete(&mut self, module: &str, line: u32, column: u32) -> Vec<LuauEntry> {
        let mut entries: Vec<LuauEntry> = Vec::new();
        let sink = LuckEntries { context: (&raw mut entries).cast(), push: push_entry };
        // SAFETY: the name and `entries` outlive the call, which copies the
        // name.
        unsafe {
            luck_checker_autocomplete(
                self.checker,
                LuckStr::new(module.as_bytes()),
                line,
                column,
                sink,
            );
        }
        entries
    }
}

impl<H> Drop for RawChecker<H> {
    fn drop(&mut self) {
        // SAFETY: the frontend is freed once, here, and nothing calls into it
        // after.
        unsafe { luck_checker_free(self.checker) };
        // SAFETY: the context came from `Box::leak` in `new`, and the frontend
        // that read it is gone.
        drop(unsafe { Box::from_raw(self.context.as_ptr()) });
    }
}

/// One piece of a check, which any thread can run.
pub(crate) struct Task(NonNull<CxxTask>);

// SAFETY: Analysis hands its tasks to other threads to run, as
// `checkQueuedModules` documents.
unsafe impl Send for Task {}

impl Task {
    pub(crate) fn run(self) {
        // SAFETY: the task is run once, here, which frees it.
        unsafe { luck_task_run(self.0) };
    }
}

/// The config Luau checks a module under, which starts as Luau's defaults,
/// for the one call to [`Host::config`] that borrows it, and which
/// [`DirectoryConfigs::apply`](crate::DirectoryConfigs::apply) sets.
#[derive(Debug)]
pub struct Config<'config> {
    raw: NonNull<CxxConfig>,
    /// The token of the check that asked for the config, a `shared_ptr` that
    /// holds none when nothing can cancel it.
    cancellation: NonNull<CxxCancellationToken>,
    call: PhantomData<&'config mut CxxConfig>,
}

impl Config<'_> {
    /// Applies the text of a `.luaurc` over what's set, as Luau's own
    /// `parseConfig` reads it, or returns Luau's error, in its own words, for
    /// the first key it can't read, where it stops with the keys before it
    /// applied.
    pub(crate) fn apply_luaurc(&mut self, text: &str) -> Result<(), String> {
        let mut errors = Vec::new();
        // SAFETY: the config is the one the frontend asked the host for, for
        // the call `self` is borrowed in, and the text and `errors` outlive
        // the call.
        unsafe {
            luck_config_apply_luaurc(
                self.raw,
                LuckStr::new(text.as_bytes()),
                LuckErrors::new(&mut errors),
            );
        }
        errors.pop().map_or(Ok(()), |error| Err(error.message))
    }

    /// Runs the source of a `.config.luau` in Luau's sandbox and applies the
    /// `luau` table it returns over what's set, as Luau's own
    /// `extractLuauConfig` does. It stops the source once it has run for a
    /// second, or once the check that asked for the config is cancelled.
    ///
    /// It returns Luau's error, in its own words, when the source doesn't run,
    /// is stopped, or returns what Luau can't read, where it stops with the
    /// keys before it applied.
    pub(crate) fn apply_luau_config(&mut self, source: &str) -> Result<(), String> {
        let mut errors = Vec::new();
        // SAFETY: as for `apply_luaurc`, and the token is the one the
        // frontend passed with the config, for the same call.
        unsafe {
            luck_config_apply_luau_config(
                self.raw,
                self.cancellation,
                LuckStr::new(source.as_bytes()),
                LuckErrors::new(&mut errors),
            );
        }
        errors.pop().map_or(Ok(()), |error| Err(error.message))
    }

    /// A copy of what's set, which [`Self::set`] sets on another module, or
    /// `None` once the check that asked for the config is cancelled, which
    /// may have stopped a `.config.luau` short.
    pub(crate) fn saved(&self) -> Option<SavedConfig> {
        if self.is_cancelled() {
            return None;
        }
        // SAFETY: the config is the one the frontend asked the host for, for
        // that call, and the copy is the `SavedConfig`'s to free.
        Some(SavedConfig { raw: unsafe { luck_config_save(self.raw) } })
    }

    /// Sets what `saved` holds in place of what's set.
    pub(crate) fn set(&mut self, saved: &SavedConfig) {
        // SAFETY: as for `apply_luaurc`, and `saved` outlives the call, which
        // copies it.
        unsafe { luck_config_set(self.raw, saved.raw) };
    }

    /// Whether the check that asked for the config is cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        // SAFETY: the token is the one the frontend passed with the config,
        // for the call `self` is borrowed in.
        unsafe { luck_cancellation_is_cancelled(self.cancellation) }
    }
}

/// A copy of a [`Config`], kept to set on other modules.
#[derive(Debug)]
pub(crate) struct SavedConfig {
    raw: NonNull<CxxConfig>,
}

// SAFETY: it points at a Luau `Config` of its own, which keeps nothing tied
// to the thread that made it.
unsafe impl Send for SavedConfig {}

impl Drop for SavedConfig {
    fn drop(&mut self) {
        // SAFETY: the copy is freed once, here.
        unsafe { luck_config_free(self.raw) };
    }
}

/// The aliases the `.config.luau` whose source is `source` declares, by
/// lowercased name, once Luau's own Config library runs it in its sandbox,
/// as `luck_resolver::LuauConfigAliases` takes them. It stops the source once
/// it has run for a second.
///
/// # Errors
///
/// Returns Luau's error, in its own words, when the source doesn't run, is
/// stopped, or returns what Luau can't read.
pub fn luau_config_aliases(source: &str) -> Result<Vec<(String, String)>, String> {
    let mut errors = Vec::new();
    let mut aliases: Vec<(String, String)> = Vec::new();
    let sink = LuckAliases { context: (&raw mut aliases).cast(), push: push_alias };
    // SAFETY: the source, `errors`, and `aliases` outlive the call.
    unsafe {
        luck_luau_config_aliases(
            LuckStr::new(source.as_bytes()),
            LuckErrors::new(&mut errors),
            sink,
        );
    }
    errors.pop().map_or(Ok(aliases), |error| Err(error.message))
}

/// Stops the checks it's passed to, from any thread. A check it stops returns
/// [`CheckError::Cancelled`](crate::CheckError::Cancelled), and the modules it
/// didn't finish are checked again by the next.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    raw: Arc<RawCancellationToken>,
}

#[derive(Debug)]
struct RawCancellationToken(NonNull<CxxCancellationToken>);

// SAFETY: it points at a `shared_ptr` to Luau's `FrontendCancellationToken`,
// which is only read, and whose one field is atomic.
unsafe impl Send for RawCancellationToken {}
// SAFETY: as for `Send`.
unsafe impl Sync for RawCancellationToken {}

impl Drop for RawCancellationToken {
    fn drop(&mut self) {
        // SAFETY: the last clone frees the `shared_ptr` once, and a check
        // holds its own copy of what it points at.
        unsafe { luck_cancellation_free(self.0) };
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self { raw: Arc::new(RawCancellationToken(luck_cancellation_new())) }
    }

    pub fn cancel(&self) {
        // SAFETY: the token lives as long as `self`.
        unsafe { luck_cancellation_cancel(self.raw.0) };
    }

    pub fn is_cancelled(&self) -> bool {
        // SAFETY: the token lives as long as `self`.
        unsafe { luck_cancellation_is_cancelled(self.raw.0) }
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Luau's built-in definitions: the declarations of the standard library
/// every checker starts with, as Luau's own `getBuiltinDefinitionSource`
/// gives them.
pub fn builtin_definitions() -> Cow<'static, str> {
    let source = luck_builtin_definitions();
    // SAFETY: the shim builds the text once and keeps it for the process.
    String::from_utf8_lossy(unsafe { source.bytes() }.unwrap_or_default())
}

/// The context and the module a host callback is for.
///
/// # Safety
///
/// `context` is the [`Context`] a [`RawChecker`] owns, which no other borrow
/// of reaches during the callback, and `module` is a string the shim passes
/// for it.
unsafe fn host_call<'call, H>(
    context: *mut c_void,
    module: LuckStr,
) -> Option<(&'call Context<H>, &'call str)> {
    // SAFETY: the caller promises it.
    let context = unsafe { &*context.cast::<Context<H>>() };
    // SAFETY: as for the context.
    let module = str::from_utf8(unsafe { module.bytes() }?).ok()?;
    Some((context, module))
}

extern "C" fn read_source<H: Host>(
    context: *mut c_void,
    module: LuckStr,
    source: NonNull<CxxString>,
) -> bool {
    // SAFETY: the shim passes back the context `RawChecker::new` gave it, on
    // the thread that called into the checker, with a name for this call.
    let Some((context, module)) = (unsafe { host_call::<H>(context, module) }) else {
        return false;
    };
    let Some(text) = context.source(module) else {
        return false;
    };
    // SAFETY: the shim passes the `std::string` for this call.
    unsafe { luck_string_assign(source, LuckStr::new(&text)) };
    true
}

extern "C" fn resolve<H: Host>(
    context: *mut c_void,
    requirer: LuckStr,
    path: LuckStr,
    module: NonNull<CxxString>,
) -> bool {
    // SAFETY: as in `read_source`.
    let Some((context, requirer)) = (unsafe { host_call::<H>(context, requirer) }) else {
        return false;
    };
    // SAFETY: the path is the require's string, which the shim passes for
    // this call.
    let path = unsafe { path.bytes() }.and_then(|bytes| str::from_utf8(bytes).ok());
    let Some(resolved) = path.and_then(|path| context.host.resolve(requirer, path)) else {
        return false;
    };
    // SAFETY: the shim passes the `std::string` for this call.
    unsafe { luck_string_assign(module, LuckStr::new(resolved.as_bytes())) };
    true
}

extern "C" fn read_config<H: Host>(
    context: *mut c_void,
    module: LuckStr,
    config: NonNull<CxxConfig>,
    cancellation: NonNull<CxxCancellationToken>,
) {
    // SAFETY: as in `read_source`.
    if let Some((context, module)) = unsafe { host_call::<H>(context, module) } {
        context.config(module, &mut Config { raw: config, cancellation, call: PhantomData });
    }
}

extern "C" fn submit_task(context: *mut c_void, task: NonNull<CxxTask>) {
    // SAFETY: the context is the `submit` that `RawChecker::check` passes,
    // which Analysis calls only on the thread that called into it.
    let submit = unsafe { &mut *context.cast::<&mut dyn FnMut(Task)>() };
    submit(Task(task));
}

extern "C" fn push_description(
    context: *mut c_void,
    location: LuckLocation,
    ty: LuckStr,
    docs: &LuckDocs,
) {
    // SAFETY: the context is the `Option` `RawChecker::describe` passes, which
    // nothing else borrows during the call.
    let described = unsafe { &mut *context.cast::<Option<LuauDescription>>() };
    *described = Some(LuauDescription {
        location,
        // SAFETY: the type is the shim's for this call.
        ty: unsafe { ty.owned() },
        // SAFETY: the docs' strings are the shim's for this call.
        docs: unsafe { docs.owned() },
        signatures: Vec::new(),
    });
}

extern "C" fn push_signature(
    context: *mut c_void,
    label: LuckStr,
    params: *const LuckRange,
    count: usize,
) {
    // SAFETY: as in `push_description`.
    let described = unsafe { &mut *context.cast::<Option<LuauDescription>>() };
    let params: &[LuckRange] = if count == 0 {
        &[]
    } else {
        // SAFETY: the shim passes `count` ranges for this call.
        unsafe { slice::from_raw_parts(params, count) }
    };
    if let Some(description) = described {
        description.signatures.push(Signature {
            // SAFETY: the label is the shim's for this call.
            label: unsafe { label.owned() },
            params: params.iter().map(|param| Span::new(param.start, param.end)).collect(),
        });
    }
}

extern "C" fn push_entry(context: *mut c_void, entry: &LuckEntry) {
    let kind = match entry.kind {
        0 => CompletionKind::Property,
        1 => CompletionKind::Binding,
        2 => CompletionKind::Keyword,
        3 => CompletionKind::String,
        4 => CompletionKind::Type,
        5 => CompletionKind::Module,
        6 => CompletionKind::GeneratedFunction,
        7 => CompletionKind::RequirePath,
        8 => CompletionKind::HotComment,
        _ => return,
    };
    // SAFETY: the context is the `Vec` `RawChecker::autocomplete` passes,
    // which nothing else borrows during the call.
    let entries = unsafe { &mut *context.cast::<Vec<LuauEntry>>() };
    entries.push(LuauEntry {
        // SAFETY: the entry's strings are the shim's for this call.
        name: unsafe { entry.name.owned() },
        kind,
        // SAFETY: as for the name.
        ty: unsafe { entry.ty.optional() },
        is_deprecated: entry.is_deprecated,
        // SAFETY: as for the name.
        insert_text: unsafe { entry.insert_text.optional() },
        // SAFETY: as for the name.
        docs: unsafe { entry.docs.owned() },
    });
}

extern "C" fn push_error(context: *mut c_void, error: &LuckError) {
    // SAFETY: the context is the `Vec` a `LuckErrors` was made over, which
    // nothing else borrows during the call.
    let errors = unsafe { &mut *context.cast::<Vec<LuauError>>() };
    // SAFETY: the error's strings are the shim's for this call.
    errors.push(unsafe { LuauError::new(error) });
}

extern "C" fn push_failure(context: *mut c_void, error: &LuckError) {
    // SAFETY: the context is the failure `RawChecker::check` passes, which
    // nothing else borrows during the call.
    let failure = unsafe { &mut *context.cast::<Option<(String, LuauError)>>() };
    // SAFETY: the error's strings are the shim's for this call.
    let module = unsafe { error.module.owned() };
    // SAFETY: as for the module.
    *failure = Some((module, unsafe { LuauError::new(error) }));
}

extern "C" fn push_alias(context: *mut c_void, name: LuckStr, value: LuckStr) {
    // SAFETY: the context is the `Vec` `luau_config_aliases` passes, which
    // nothing else borrows during the call.
    let aliases = unsafe { &mut *context.cast::<Vec<(String, String)>>() };
    // SAFETY: the strings are the shim's for this call.
    let name = unsafe { name.owned() };
    // SAFETY: as for the name.
    let value = unsafe { value.owned() };
    aliases.push((name, value));
}
