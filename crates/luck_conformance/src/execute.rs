//! The reference interpreter. A program runs in a child process of its own,
//! so one that exits, loops, changes the locale, or crashes takes nothing
//! else down.

use std::fs;
use std::io::{self, Read as _};
use std::path::Path;

use mlua::thread::ThreadStatus;
use mlua::{Function, Lua, LuaOptions, LuaString, MultiValue, StdLib, Value};

use crate::suite::{FAST_FLAGS, check_interpreter};

/// The argument that makes this binary run the program on its standard input
/// instead of a suite.
pub(crate) const MODE: &str = "execute";

/// Twice the 1 GiB buffer `buffers.luau` makes, and a bound on the tests that
/// exhaust memory on purpose.
const MEMORY_LIMIT_BYTES: usize = 1 << 31;

/// How a program ended in the interpreter, which the child writes to its
/// verdict file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// What the program returned, one value per line.
    Returned(String),
    Failed(String),
}

/// A verdict file starts with one of these lines, and the values or the
/// error follow it.
const RETURNED: &str = "returned";
const FAILED: &str = "failed";

impl Verdict {
    fn write(&self, path: &Path) -> io::Result<()> {
        let (kind, payload) = match self {
            Self::Returned(values) => (RETURNED, values),
            Self::Failed(error) => (FAILED, error),
        };
        fs::write(path, format!("{kind}\n{payload}"))
    }

    /// The verdict a child wrote to `path`, if it wrote one.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let verdict = fs::read_to_string(path).ok()?;
        let (kind, payload) = verdict.split_once('\n').unwrap_or((&verdict, ""));
        match kind {
            RETURNED => Some(Self::Returned(payload.to_owned())),
            FAILED => Some(Self::Failed(payload.to_owned())),
            _ => None,
        }
    }
}

/// Sets up the interpreter, before anything else in this process uses it,
/// and checks it's the release the suite is pinned to.
///
/// Luau reads its flags when a state starts or code compiles, so they're set
/// before either.
pub(crate) fn start() -> io::Result<()> {
    for flag in FAST_FLAGS {
        Lua::set_fflag(flag, true)
            .map_err(|()| io::Error::other(format!("Luau has no flag `{flag}`")))?;
    }
    let interpreter: String = Lua::new()
        .globals()
        .get("_VERSION")
        .map_err(|error| io::Error::other(error.to_string()))?;
    check_interpreter(&interpreter)
}

/// Whether the interpreter compiles `program`, which decides whether luck
/// should parse it.
pub(crate) fn compiles(program: &[u8]) -> bool {
    Lua::new().load(program).into_function().is_ok()
}

/// Runs the program on standard input as the chunk `chunk_name`, set up as
/// Luau's runner sets up a test, in the current directory, and writes its
/// [`Verdict`] to `verdict_path`.
pub(crate) fn run(chunk_name: &str, verdict_path: &Path) -> io::Result<()> {
    let mut program = Vec::new();
    io::stdin().read_to_end(&mut program)?;
    execute(chunk_name, program).write(verdict_path)
}

/// As the `runConformance` of Luau's `Conformance.test.cpp` runs a test,
/// without the host functions single tests register.
fn execute(chunk_name: &str, program: Vec<u8>) -> Verdict {
    // SAFETY: The suites need the `debug` library, which mlua counts as
    // unsafe because Lua code can use it to break mlua's invariants. The
    // programs are the suites' own tests and luck's output for them, and each
    // runs alone in this process, so one that breaks the state takes down only
    // its own run, which the parent reports.
    #[expect(unsafe_code, reason = "the suites' tests need the `debug` library")]
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) };
    let ran = lua.set_memory_limit(MEMORY_LIMIT_BYTES).and_then(|_unlimited| {
        let main = lua.load(program).set_name(chunk_name);
        let globals = lua.globals();
        // The runner's native-code helpers, as they answer with native code
        // generation off.
        globals.set("is_native", lua.create_function(|_, ()| Ok(false))?)?;
        globals.set("is_native_if_supported", lua.create_function(|_, ()| Ok(true))?)?;
        globals.set("loadstring", lua.create_function(loadstring)?)?;
        // The test gets globals of its own over the shared ones, as
        // `luaL_sandboxthread` gives it, which `_G` names. A thread starts
        // with the globals of the one that made it.
        let environment = lua.create_table()?;
        environment.set_metatable(Some(lua.create_table_from([("__index", &globals)])?))?;
        environment.set("_G", &environment)?;
        globals.get::<Function>("setfenv")?.call::<()>((0, &environment))?;
        // Luau's runner resumes each test as a thread, so its main chunk can
        // yield to it.
        let thread = lua.create_thread(main.set_environment(environment).into_function()?)?;
        let values: MultiValue = thread.resume(())?;
        if thread.status() == ThreadStatus::Resumable {
            return Err(mlua::Error::runtime("thread yielded unexpectedly"));
        }
        Ok(values)
    });
    // The values are only readable while `lua` lives.
    match ran {
        Ok(values) => Verdict::Returned(values.iter().map(render).collect::<Vec<_>>().join("\n")),
        Err(error) => Verdict::Failed(message(&error)),
    }
}

/// Luau's runner's `loadstring`, which names a chunk after its source unless
/// it's given a name, and returns `nil` and the message when the source
/// doesn't compile.
fn loadstring(
    lua: &Lua,
    (source, chunk_name): (LuaString, Option<LuaString>),
) -> mlua::Result<(Value, Value)> {
    let name = chunk_name.as_ref().unwrap_or(&source).to_string_lossy();
    let bytes = source.as_bytes();
    match lua.load(bytes.as_ref()).set_name(name).into_function() {
        Ok(function) => Ok((Value::Function(function), Value::Nil)),
        Err(mlua::Error::SyntaxError { message, .. }) => {
            Ok((Value::Nil, Value::String(lua.create_string(message)?)))
        }
        Err(error) => Err(error),
    }
}

/// A table's or function's text holds its address, which differs between
/// runs, so only its type is compared.
fn render(value: &Value) -> String {
    if matches!(
        value,
        Value::Nil | Value::Boolean(_) | Value::Integer(_) | Value::Number(_) | Value::String(_)
    ) {
        value.to_string().unwrap_or_else(|error| error.to_string())
    } else {
        value.type_name().to_owned()
    }
}

/// The error without the traceback mlua appends, whose lines differ wherever
/// the program's lines do.
fn message(error: &mlua::Error) -> String {
    let text = error.to_string();
    text.split_once("\nstack traceback:")
        .map_or(text.as_str(), |(head, _traceback)| head)
        .to_owned()
}
