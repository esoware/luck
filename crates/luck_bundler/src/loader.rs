use std::fmt::Write as _;

/// Writes the loader a bundle opens with, then the runtime its value-export
/// modules call for when `has_export_runtime`.
pub(crate) fn write_prelude(prefix: &str, has_export_runtime: bool, output: &mut String) {
    write_loader(prefix, output);
    if has_export_runtime {
        write_export_runtime(prefix, output);
    }
}

/// The cache holds only a non-nil result, so a raising or nil-returning
/// module runs again on the next require.
///
/// Chunks receive no arguments and must return exactly one value
/// (`Require/src/RequireImpl.cpp`, `CLI/src/ReplRequirer.cpp`).
fn write_loader(prefix: &str, output: &mut String) {
    let _ = write!(
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

/// The runtime behind value-export modules, which a bundle needs only when
/// one of its modules exports a value.
///
/// `{prefix}exporting` hands a cyclic requirer a placeholder whose field
/// accesses error until the module finishes, then fills and freezes it
/// (`populatePlaceholder` in `Require/src/RequireImpl.cpp`).
fn write_export_runtime(prefix: &str, output: &mut String) {
    let _ = write!(
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
