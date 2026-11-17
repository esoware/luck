use core::fmt::Write as _;

use luck_token::LuaVersion;

/// What every generated name's prefix starts with: `__luck_`, unless a
/// module's source holds that, then `__luck1_`, `__luck2_`, and so on.
pub(crate) const PREFIX_STEM: &str = "__luck";

/// What decides the code a bundle opens with, apart from its prefix.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoaderShape {
    pub version: LuaVersion,
    /// True only for a 5.1 bundle with a function that has no upvalue left
    /// for a local loader.
    ///
    /// A global read costs none, but two bundles in one Lua state then
    /// overwrite each other's loader.
    pub is_loader_global: bool,
    /// Only Lua targets record dynamic callees, so only a Lua loader gets the
    /// entry point they're retargeted at.
    pub has_dynamic_entry: bool,
    pub has_export_runtime: bool,
}

/// Writes the loader a bundle opens with, then the entry point and the
/// runtime its modules call for.
pub(crate) fn write_prelude(prefix: &str, shape: LoaderShape, output: &mut String) {
    write_loader(prefix, shape.version, shape.is_loader_global, output);
    if shape.has_dynamic_entry {
        write_dynamic_entry(prefix, shape.is_loader_global, output);
    }
    if shape.has_export_runtime {
        write_export_runtime(prefix, output);
    }
}

fn write_loader(prefix: &str, version: LuaVersion, is_loader_global: bool, output: &mut String) {
    if version.is_luau() {
        write_luau_loader(prefix, output);
        return;
    }

    // The cache is package.loaded itself when it exists, and package.preload
    // wins over bundled files, matching real require's state and searcher
    // order. The require's truthiness check reloads a module that returned
    // false, as every PUC version does.
    let _infallible = write!(
        output,
        "local {prefix}modules={{}}\n\
         local {prefix}loaded=type(package)==\"table\" and type(package.loaded)==\"table\" and package.loaded or {{}}\n\
         local {prefix}preload=type(package)==\"table\" and type(package.preload)==\"table\" and package.preload or {{}}\n"
    );
    if version.has_require_filepath_arg() {
        output.extend(["local ", prefix, "paths={}\n"]);
    }
    write_lua_require(prefix, version, is_loader_global, output);
}

/// What a loader function's declaration opens and closes with.
///
/// A global one is declared inside `do ... end`, off the bundle's top level,
/// where the minifier's `rename_globals` finds the globals a file defines, so
/// it keeps its prefixed name, which no host global shares.
const fn entry_point(is_loader_global: bool) -> (&'static str, &'static str) {
    if is_loader_global { ("do function", " end") } else { ("local function", "") }
}

/// The cache holds only a non-nil result, so a raising or nil-returning
/// module runs again on the next require.
///
/// Chunks receive no arguments and must return exactly one value
/// (`Require/src/RequireImpl.cpp`, `CLI/src/ReplRequirer.cpp`).
fn write_luau_loader(prefix: &str, output: &mut String) {
    let _infallible = write!(
        output,
        "local {prefix}modules={{}}\n\
         local {prefix}loaded={{}}\n\
         local function {prefix}result(...)\n\
         if select(\"#\",...)~=1 then\n\
         error(\"luck bundle: module must return a single value\",0)\n\
         end\n\
         return ...\n\
         end\n\
         local function {prefix}require(id)\n\
         local value={prefix}loaded[id]\n\
         if value~=nil then return value end\n\
         value={prefix}result({prefix}modules[id]())\n\
         {prefix}loaded[id]=value\n\
         return value\n\
         end\n"
    );
}

/// The runtime behind Luau value-export modules, which a bundle needs only
/// when one of its modules exports a value.
///
/// `{prefix}exporting` hands a cyclic requirer a placeholder whose field
/// accesses error until the module finishes, then fills and freezes it
/// (`populatePlaceholder` in `Require/src/RequireImpl.cpp`).
fn write_export_runtime(prefix: &str, output: &mut String) {
    let _infallible = write!(
        output,
        "local {prefix}freeze=table.freeze\n\
         local {prefix}lock={{\n\
         __index=function(_,key)error(\"luck bundle: cannot read exported field '\"..tostring(key)..\"' of a module still loading in a require cycle\",2)end,\n\
         __newindex=function(_,key)error(\"luck bundle: cannot set exported field '\"..tostring(key)..\"' of a module still loading in a require cycle\",2)end,\n\
         }}\n\
         local function {prefix}exporting(id,body)\n\
         return function()\n\
         local placeholder=setmetatable({{}},{prefix}lock)\n\
         {prefix}loaded[id]=placeholder\n\
         local exports=body()\n\
         setmetatable(placeholder,nil)\n\
         for key,value in exports do placeholder[key]=value end\n\
         return {prefix}freeze(placeholder)\n\
         end\n\
         end\n"
    );
}

fn write_lua_require(
    prefix: &str,
    version: LuaVersion,
    is_loader_global: bool,
    output: &mut String,
) {
    if !version.has_require_error_retry() {
        // 5.1: a sentinel marks in-progress loads, and hitting it (a cycle, or
        // a module whose load previously raised) reproduces 5.1's "loop or
        // previous error" failure. It's a function because `module()` adopts
        // a table it finds in `package.loaded`.
        let (opening, closing) = entry_point(is_loader_global);
        let _infallible = write!(
            output,
            "local {prefix}loading=function()end\n\
             {opening} {prefix}require(name)\n\
             local value={prefix}loaded[name]\n\
             if value then\n\
             if value=={prefix}loading then error(\"loop or previous error loading module '\"..name..\"'\",2)end\n\
             return value\n\
             end\n\
             local loader={prefix}preload[name] or {prefix}modules[name]\n\
             {prefix}loaded[name]={prefix}loading\n\
             value=loader(name)\n\
             if value~=nil then {prefix}loaded[name]=value end\n\
             value={prefix}loaded[name]\n\
             if value=={prefix}loading then\n\
             value=true\n\
             {prefix}loaded[name]=value\n\
             end\n\
             return value\n\
             end{closing}\n"
        );
        return;
    }
    if !version.has_require_loaderdata() {
        // 5.2/5.3 have no sentinel. As in real 5.2+, a load-time cycle
        // recurses until the stack overflows, and a load that raises
        // leaves the cache unset so a later require retries.
        let _infallible = write!(
            output,
            "local function {prefix}require(name)\n\
             local value={prefix}loaded[name]\n\
             if value then return value end\n\
             local preloader={prefix}preload[name]\n\
             if preloader then\n\
             value=preloader(name,nil)\n\
             else\n\
             value={prefix}modules[name](name,{prefix}paths[name])\n\
             end\n\
             if value~=nil then {prefix}loaded[name]=value end\n\
             value={prefix}loaded[name]\n\
             if value==nil then\n\
             value=true\n\
             {prefix}loaded[name]=value\n\
             end\n\
             return value\n\
             end\n"
        );
        return;
    }
    // 5.4/5.5 also return the loader data as a second result, on first
    // load only.
    let _infallible = write!(
        output,
        "local function {prefix}require(name)\n\
         local value={prefix}loaded[name]\n\
         if value then return value end\n\
         local loaderdata\n\
         local loader={prefix}preload[name]\n\
         if loader then\n\
         loaderdata=\":preload:\"\n\
         else\n\
         loader={prefix}modules[name]\n\
         loaderdata={prefix}paths[name]\n\
         end\n\
         value=loader(name,loaderdata)\n\
         if value~=nil then {prefix}loaded[name]=value end\n\
         value={prefix}loaded[name]\n\
         if value==nil then\n\
         value=true\n\
         {prefix}loaded[name]=value\n\
         end\n\
         return value,loaderdata\n\
         end\n"
    );
}

/// The Lua loader's entry point for retargeted `require(expr)` calls. It
/// serves a runtime-computed name from the bundle when it's bundled or
/// preloaded, since the Lua cache is keyed by module name.
///
/// Anything else goes to the host's own `require`, looked up at call time so
/// one the host installs later still counts.
fn write_dynamic_entry(prefix: &str, is_loader_global: bool, output: &mut String) {
    let (opening, closing) = entry_point(is_loader_global);
    let _infallible = write!(
        output,
        "{opening} {prefix}dynamic(name)\n\
         if {prefix}modules[name]~=nil or {prefix}preload[name]~=nil then return {prefix}require(name)end\n\
         if type(require)==\"function\" then return require(name)end\n\
         error(\"luck bundle: dynamic require of '\"..tostring(name)..\"' is not in the bundle and this runtime has no require\",2)\n\
         end{closing}\n"
    );
}
