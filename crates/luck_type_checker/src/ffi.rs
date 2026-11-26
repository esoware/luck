//! The C ABI of `shim.cpp`, and the safe types over it.
//!
//! Analysis calls a host's callbacks only on the thread that called into the
//! checker, since `Frontend::checkQueuedModules` reads, parses, and resolves
//! the whole module graph before it hands out a task, and a task asks the
//! host nothing. So a context is read on one thread at a time. A Rust panic
//! in a callback aborts, since an `extern "C"` function can't unwind.

#![expect(
    unsafe_code,
    reason = "the type checker is Luau's C++, which only a foreign call reaches"
)]

use std::borrow::Cow;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;
use std::{slice, str};

use crate::checker::{Context, Host};

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

    /// # Safety
    ///
    /// `data` points at `len` bytes that stay put and unchanged for `'bytes`,
    /// or `len` is 0.
    unsafe fn bytes<'bytes>(self) -> &'bytes [u8] {
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the caller promises `len` readable bytes at `data`.
        unsafe { slice::from_raw_parts(self.data, self.len) }
    }
}

#[repr(C)]
struct LuckHost {
    context: *mut c_void,
    source: extern "C" fn(context: *mut c_void, module: LuckStr, source: *mut c_void) -> bool,
    resolve: extern "C" fn(
        context: *mut c_void,
        requirer: LuckStr,
        path: LuckStr,
        module: *mut c_void,
    ) -> bool,
    config: extern "C" fn(context: *mut c_void, module: LuckStr, config: *mut c_void),
}

#[repr(C)]
struct LuckError {
    module: usize,
    is_syntax_error: bool,
    has_location: bool,
    begin_line: u32,
    begin_column: u32,
    end_line: u32,
    end_column: u32,
    module_name: LuckStr,
    message: LuckStr,
}

#[repr(C)]
struct LuckErrors {
    context: *mut c_void,
    push: extern "C" fn(context: *mut c_void, error: *const LuckError),
}

#[repr(C)]
struct LuckExecutor {
    context: *mut c_void,
    submit: extern "C" fn(context: *mut c_void, task: *mut c_void),
}

const LUCK_CANCELLED: u8 = 1;
const LUCK_INTERNAL_ERROR: u8 = 2;

unsafe extern "C" {
    fn luck_string_assign(string: *mut c_void, value: LuckStr);
    fn luck_config_apply_luaurc(config: *mut c_void, text: LuckStr, errors: LuckErrors);
    fn luck_checker_new(host: LuckHost) -> *mut c_void;
    fn luck_checker_free(checker: *mut c_void);
    fn luck_checker_load_definitions(
        checker: *mut c_void,
        name: LuckStr,
        source: LuckStr,
        errors: LuckErrors,
    );
    fn luck_checker_mark_dirty(checker: *mut c_void, module: LuckStr);
    fn luck_checker_check(
        checker: *mut c_void,
        modules: *const LuckStr,
        count: usize,
        cancellation: *const c_void,
        executor: LuckExecutor,
        errors: LuckErrors,
    ) -> u8;
    fn luck_task_run(task: *mut c_void);
    safe fn luck_cancellation_new() -> *mut c_void;
    fn luck_cancellation_cancel(token: *const c_void);
    fn luck_cancellation_free(token: *mut c_void);
    safe fn luck_builtin_definitions() -> LuckStr;
}

/// An error Analysis reported, with its location as Luau counts it: lines
/// and byte columns from 0.
pub(crate) struct LuauError {
    /// The index of the checked module it's in.
    pub(crate) module: usize,
    pub(crate) module_name: String,
    pub(crate) is_syntax_error: bool,
    pub(crate) location: Option<[u32; 4]>,
    pub(crate) message: String,
}

pub(crate) enum Outcome {
    /// The errors in the modules checked, in no order.
    Checked(Vec<LuauError>),
    Cancelled,
    /// Analysis threw this `InternalCompilerError`.
    InternalError(LuauError),
}

/// A frontend over the host in a [`Context`] it owns.
pub(crate) struct RawChecker<H> {
    checker: *mut c_void,
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
        let sink = LuckErrors { context: (&raw mut errors).cast(), push: push_error };
        // SAFETY: the strings and `errors` outlive the call, which copies the
        // strings.
        unsafe {
            luck_checker_load_definitions(
                self.checker,
                LuckStr::new(name.as_bytes()),
                LuckStr::new(source.as_bytes()),
                sink,
            );
        }
        errors
    }

    pub(crate) fn mark_dirty(&mut self, module: &str) {
        // SAFETY: the name outlives the call, which copies it.
        unsafe { luck_checker_mark_dirty(self.checker, LuckStr::new(module.as_bytes())) };
    }

    /// Checks `modules`, handing each task Analysis splits the work into to
    /// `submit`, and returns once every task has run.
    pub(crate) fn check(
        &mut self,
        modules: &[&str],
        cancellation: &CancellationToken,
        mut submit: &mut dyn FnMut(Task),
    ) -> Outcome {
        let names: Vec<LuckStr> =
            modules.iter().map(|module| LuckStr::new(module.as_bytes())).collect();
        let mut errors = Vec::new();
        let executor = LuckExecutor { context: (&raw mut submit).cast(), submit: submit_task };
        let sink = LuckErrors { context: (&raw mut errors).cast(), push: push_error };
        // SAFETY: the names, the token, `submit`, and `errors` all outlive the
        // call, which waits for every task it submits.
        let outcome = unsafe {
            luck_checker_check(
                self.checker,
                names.as_ptr(),
                names.len(),
                cancellation.raw.0,
                executor,
                sink,
            )
        };
        match (outcome, errors.pop()) {
            (LUCK_CANCELLED, _) => Outcome::Cancelled,
            (LUCK_INTERNAL_ERROR, Some(error)) => Outcome::InternalError(error),
            (_, last) => {
                errors.extend(last);
                Outcome::Checked(errors)
            }
        }
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
pub(crate) struct Task(NonNull<c_void>);

// SAFETY: Analysis hands its tasks to other threads to run, as
// `checkQueuedModules` documents.
unsafe impl Send for Task {}

impl Task {
    pub(crate) fn run(self) {
        // SAFETY: the task is run once, here, which frees it.
        unsafe { luck_task_run(self.0.as_ptr()) };
    }
}

/// The config Luau checks a module under, which starts as Luau's defaults,
/// for the one call to [`Host::config`] that borrows it.
#[derive(Debug)]
pub struct Config<'config> {
    config: NonNull<c_void>,
    call: PhantomData<&'config mut c_void>,
}

impl Config<'_> {
    /// Applies the text of a `.luaurc` over what's set, as Luau's own
    /// `parseConfig` reads it.
    ///
    /// # Errors
    ///
    /// Returns Luau's error, in its own words, for the first key it can't
    /// read, where it stops with the keys before it applied.
    pub fn apply_luaurc(&mut self, text: &str) -> Result<(), String> {
        let mut errors: Vec<LuauError> = Vec::new();
        let sink = LuckErrors { context: (&raw mut errors).cast(), push: push_error };
        // SAFETY: the config is the one the frontend asked the host for, for
        // the call `self` is borrowed in, and the text and `errors` outlive
        // the call.
        unsafe {
            luck_config_apply_luaurc(self.config.as_ptr(), LuckStr::new(text.as_bytes()), sink);
        }
        errors.pop().map_or(Ok(()), |error| Err(error.message))
    }
}

/// Stops the checks it's passed to, from any thread. A check it stops returns
/// [`CheckError::Cancelled`](crate::CheckError::Cancelled), and the modules it
/// didn't finish are checked again by the next.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    raw: Arc<RawCancellationToken>,
}

#[derive(Debug)]
struct RawCancellationToken(*mut c_void);

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
    String::from_utf8_lossy(unsafe { source.bytes() })
}

/// # Safety
///
/// `context` is the [`Context`] a [`RawChecker`] owns, which no other borrow
/// of reaches during the callback.
unsafe fn context<'context, H>(context: *mut c_void) -> &'context Context<H> {
    // SAFETY: the caller promises it.
    unsafe { &*context.cast::<Context<H>>() }
}

/// # Safety
///
/// `name` is a string the shim passes for the callback it's in.
unsafe fn module_name<'name>(name: LuckStr) -> Option<&'name str> {
    // SAFETY: the caller promises it.
    str::from_utf8(unsafe { name.bytes() }).ok()
}

extern "C" fn read_source<H: Host>(
    context_ptr: *mut c_void,
    module: LuckStr,
    source_string: *mut c_void,
) -> bool {
    // SAFETY: the shim passes back the context `RawChecker::new` gave it, on
    // the thread that called into the checker.
    let context = unsafe { context::<H>(context_ptr) };
    // SAFETY: the name is the shim's for this call.
    let Some(module) = (unsafe { module_name(module) }) else {
        return false;
    };
    let Some(source) = context.source(module) else {
        return false;
    };
    // SAFETY: the shim passes the `std::string` for this call.
    unsafe { luck_string_assign(source_string, LuckStr::new(&source)) };
    true
}

extern "C" fn resolve<H: Host>(
    context_ptr: *mut c_void,
    requirer: LuckStr,
    path: LuckStr,
    module_string: *mut c_void,
) -> bool {
    // SAFETY: as in `read_source`.
    let context = unsafe { context::<H>(context_ptr) };
    // SAFETY: the name is the shim's for this call.
    let Some(requirer) = (unsafe { module_name(requirer) }) else {
        return false;
    };
    // SAFETY: the path is the require's string, which the shim passes for
    // this call.
    let Ok(path) = str::from_utf8(unsafe { path.bytes() }) else {
        return false;
    };
    let Some(module) = context.host.resolve(requirer, path) else {
        return false;
    };
    // SAFETY: the shim passes the `std::string` for this call.
    unsafe { luck_string_assign(module_string, LuckStr::new(module.as_bytes())) };
    true
}

extern "C" fn read_config<H: Host>(context_ptr: *mut c_void, module: LuckStr, config: *mut c_void) {
    // SAFETY: as in `read_source`.
    let context = unsafe { context::<H>(context_ptr) };
    // SAFETY: the name is the shim's for this call.
    let (Some(module), Some(config)) = (unsafe { module_name(module) }, NonNull::new(config))
    else {
        return;
    };
    context.host.config(module, &mut Config { config, call: PhantomData });
}

extern "C" fn submit_task(context: *mut c_void, task: *mut c_void) {
    // SAFETY: the context is the `submit` that `RawChecker::check` passes,
    // which Analysis calls only on the thread that called into it.
    let submit = unsafe { &mut *context.cast::<&mut dyn FnMut(Task)>() };
    if let Some(task) = NonNull::new(task) {
        submit(Task(task));
    }
}

extern "C" fn push_error(context: *mut c_void, error: *const LuckError) {
    // SAFETY: the context is the `Vec` a `RawChecker` method passes, which
    // nothing else borrows during the call.
    let errors = unsafe { &mut *context.cast::<Vec<LuauError>>() };
    // SAFETY: the shim passes an error it built for this call.
    let error = unsafe { &*error };
    // SAFETY: the error's strings are the shim's for this call.
    let module_name = unsafe { error.module_name.bytes() };
    // SAFETY: as for the module's name.
    let message = unsafe { error.message.bytes() };
    errors.push(LuauError {
        module: error.module,
        module_name: String::from_utf8_lossy(module_name).into_owned(),
        is_syntax_error: error.is_syntax_error,
        location: error.has_location.then_some([
            error.begin_line,
            error.begin_column,
            error.end_line,
            error.end_column,
        ]),
        message: String::from_utf8_lossy(message).into_owned(),
    });
}
