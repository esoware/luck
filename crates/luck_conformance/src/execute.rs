//! The reference interpreter. A program runs in a child process of its own,
//! so one that exits, loops, changes the locale, or crashes takes nothing
//! else down.

use std::fs;
use std::io::{self, Read as _};
use std::path::Path;

use mlua::thread::ThreadStatus;
use mlua::{Function, Lua, LuaOptions, LuaString, MultiValue, StdLib, Value};

use crate::suite::{Runner, Suite};

/// The argument that makes this binary run the program on its standard input
/// instead of a suite.
pub(crate) const MODE: &str = "execute";

/// A verdict file starts with one of these lines.
pub(crate) const RETURNED: &str = "returned";
pub(crate) const FAILED: &str = "failed";

/// Twice the 1 GiB buffer `buffers.luau` makes, and a bound on the tests that
/// exhaust memory on purpose.
const MEMORY_LIMIT_BYTES: usize = 1 << 31;

/// Sets up the interpreter for `suite`, before anything else in this process
/// uses it, and checks it's the release the suite is pinned to.
///
/// # Errors
///
/// Fails when a flag doesn't exist, or the interpreter isn't the pinned
/// release.
pub(crate) fn start(suite: &Suite) -> io::Result<()> {
    enable_fast_flags(suite)?;
    let interpreter: String = Lua::new()
        .globals()
        .get("_VERSION")
        .map_err(|error| io::Error::other(error.to_string()))?;
    suite.check_interpreter(&interpreter)
}

/// Luau reads its flags when a state starts or code compiles, so they're set
/// before either.
///
/// # Errors
///
/// Fails when Luau has no flag by one of the names.
#[cfg(feature = "luau")]
fn enable_fast_flags(suite: &Suite) -> io::Result<()> {
    for flag in suite.fast_flags {
        Lua::set_fflag(flag, true)
            .map_err(|()| io::Error::other(format!("Luau has no flag `{flag}`")))?;
    }
    Ok(())
}

/// # Errors
///
/// Fails when the suite names a flag, which only Luau has.
#[cfg(not(feature = "luau"))]
fn enable_fast_flags(suite: &Suite) -> io::Result<()> {
    match suite.fast_flags {
        [] => Ok(()),
        [flag, ..] => Err(io::Error::other(format!("only Luau has flags, such as `{flag}`"))),
    }
}

/// Whether the interpreter compiles `program`, which decides whether luck
/// should parse it.
pub(crate) fn compiles(program: &[u8]) -> bool {
    Lua::new().load(program).into_function().is_ok()
}

/// Runs the program on standard input as the chunk `chunk_name` of `suite`,
/// in the current directory.
///
/// It writes how the program ended to `verdict_path`: the [`RETURNED`] line
/// and each value it returned on a line of its own, or the [`FAILED`] line and
/// the error.
///
/// # Errors
///
/// Fails when standard input can't be read or the verdict can't be written.
pub(crate) fn run(suite: &Suite, chunk_name: &str, verdict_path: &Path) -> io::Result<()> {
    let mut program = Vec::new();
    io::stdin().read_to_end(&mut program)?;
    fs::write(verdict_path, execute(suite, chunk_name, program))
}

fn execute(suite: &Suite, chunk_name: &str, program: Vec<u8>) -> String {
    // SAFETY: The suites need the `debug` library, which mlua counts as
    // unsafe because Lua code can use it to break mlua's invariants. The
    // programs are the suites' own tests and luck's output for them, and each
    // runs alone in this process, so one that breaks the state takes down only
    // its own run, which the parent reports.
    #[expect(unsafe_code, reason = "the suites' tests need the `debug` library")]
    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL, LuaOptions::default()) };
    let ran = lua.set_memory_limit(MEMORY_LIMIT_BYTES).and_then(|_unlimited| {
        let main = lua.load(program).set_name(chunk_name);
        match suite.runner {
            Runner::PucRio => {
                // Skip the tests that take long or much memory, the ones
                // whose results differ between platforms, and the messages
                // about tests skipped. Lua 5.2's suite keeps two kinds of
                // the second apart: the POSIX ones, and those that need a
                // build with `long long`.
                let globals = lua.globals();
                for global in ["_soft", "_port", "_nomsg", "_noposix", "_nolonglong"] {
                    globals.set(global, true)?;
                }
                main.call::<MultiValue>(())
            }
            Runner::Luau => {
                let globals = lua.globals();
                // The runner's native-code helpers, as they answer with
                // native code generation off.
                globals.set("is_native", lua.create_function(|_, ()| Ok(false))?)?;
                globals.set("is_native_if_supported", lua.create_function(|_, ()| Ok(true))?)?;
                globals.set("loadstring", lua.create_function(loadstring)?)?;
                // The test gets globals of its own over the shared ones, as
                // `luaL_sandboxthread` gives it, which `_G` names. A thread
                // starts with the globals of the one that made it.
                let environment = lua.create_table()?;
                environment.set_metatable(Some(lua.create_table_from([("__index", &globals)])?))?;
                environment.set("_G", &environment)?;
                globals.get::<Function>("setfenv")?.call::<()>((0i32, &environment))?;
                // Luau's runner resumes each test as a thread, so its main
                // chunk can yield to it.
                let thread =
                    lua.create_thread(main.set_environment(environment).into_function()?)?;
                let values = thread.resume(())?;
                if thread.status() == ThreadStatus::Resumable {
                    return Err(mlua::Error::runtime("thread yielded unexpectedly"));
                }
                Ok(values)
            }
        }
    });
    // The values are only readable while `lua` lives.
    match ran {
        Ok(values) => values.iter().fold(String::from(RETURNED), |mut verdict, value| {
            verdict.push('\n');
            verdict.push_str(&render(value));
            verdict
        }),
        Err(error) => format!("{FAILED}\n{}", message(&error)),
    }
}

/// Luau's runner's `loadstring`, which names a chunk after its source unless
/// it's given a name, and returns `nil` and the message when the source
/// doesn't compile.
///
/// # Errors
///
/// Fails when the interpreter does, as when it runs out of memory.
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
