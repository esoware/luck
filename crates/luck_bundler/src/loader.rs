//! The memoizing loader at the top of a bundle, which reproduces the target
//! version's own `require` (`ll_require` in `loadlib.c`).

use luck_token::LuaVersion;

/// The loader, with every generated name starting with `prefix`.
pub(crate) fn loader_text(prefix: &str, version: LuaVersion) -> String {
    if version.is_luau() {
        return luau_loader_text(prefix);
    }

    // The cache is package.loaded itself when it exists, and package.preload
    // wins over bundled files, matching real require's state and searcher
    // order. The require's truthiness check reloads a module that returned
    // false, as every PUC version does.
    let cache_tables = format!(
        "local {prefix}modules={{}}\n\
         local {prefix}loaded=type(package)==\"table\" and type(package.loaded)==\"table\" and package.loaded or {{}}\n\
         local {prefix}preload=type(package)==\"table\" and type(package.preload)==\"table\" and package.preload or {{}}\n"
    );
    let paths_table = if version.has_require_filepath_arg() {
        format!("local {prefix}paths={{}}\n")
    } else {
        String::new()
    };
    [cache_tables, paths_table, lua_require_text(prefix, version)].concat()
}

/// Luau/Roblox require: private cache keyed by file, cycles and repeated
/// failures raise, modules must return exactly one non-nil value, chunks
/// receive no arguments.
fn luau_loader_text(prefix: &str) -> String {
    format!(
        "local {prefix}modules={{}}\n\
         local {prefix}loaded={{}}\n\
         local {prefix}loading={{}}\n\
         local function {prefix}result(...)\n\
         if select(\"#\",...)~=1 or (...)==nil then\n\
         error(\"luck bundle: module must return exactly one non-nil value\",0)\n\
         end\n\
         return ...\n\
         end\n\
         local function {prefix}require(id)\n\
         local value={prefix}loaded[id]\n\
         if value~=nil then\n\
         if value=={prefix}loading then error(\"luck bundle: require cycle hit while loading module #\"..id,2)end\n\
         return value\n\
         end\n\
         {prefix}loaded[id]={prefix}loading\n\
         value={prefix}result({prefix}modules[id]())\n\
         {prefix}loaded[id]=value\n\
         return value\n\
         end\n"
    )
}

fn lua_require_text(prefix: &str, version: LuaVersion) -> String {
    if !version.has_require_error_retry() {
        // 5.1: a sentinel marks in-progress loads; hitting it (a cycle,
        // or a module whose load previously raised) reproduces 5.1's
        // "loop or previous error" failure.
        return format!(
            "local {prefix}loading={{}}\n\
             local function {prefix}require(name)\n\
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
             end\n"
        );
    }
    if !version.has_require_loaderdata() {
        // 5.2/5.3 have no sentinel. As in real 5.2+, a load-time cycle
        // recurses until the stack overflows, and a load that raises
        // leaves the cache unset so a later require retries.
        return format!(
            "local function {prefix}require(name)\n\
             local value={prefix}loaded[name]\n\
             if value then return value end\n\
             local preloader={prefix}preload[name]\n\
             if preloader then\n\
             value=preloader(name)\n\
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
    }
    // 5.4/5.5 also return the loader data as a second result, on first
    // load only.
    format!(
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
    )
}

/// The Lua loader's entry point for retargeted `require(expr)` calls. It
/// serves a runtime-computed name from the bundle when it's bundled or
/// preloaded, since the Lua cache is keyed by module name.
///
/// Anything else goes to the host's own `require`, looked up at call time so
/// one the host installs later still counts.
pub(crate) fn dynamic_entry_text(prefix: &str) -> String {
    format!(
        "local function {prefix}dynamic(name)\n\
         if {prefix}modules[name]~=nil or {prefix}preload[name]~=nil then return {prefix}require(name)end\n\
         if type(require)==\"function\" then return require(name)end\n\
         error(\"luck bundle: dynamic require of '\"..tostring(name)..\"' is not in the bundle and this runtime has no require\",2)\n\
         end\n"
    )
}
