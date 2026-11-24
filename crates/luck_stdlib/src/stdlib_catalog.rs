//! The built-in libraries, one static library per target.
//!
//! Each is an expression `just stdlib` generates from `stdlib_data/` by
//! layering its files as std definitions over an empty library and printing
//! the result, so nothing is parsed or checked at runtime. It names what it
//! uses from this module.

use luck_token::LuaTarget;

use crate::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibReplacement, StdlibShape, StdlibSignature, StdlibValue,
    stdlib_map,
};

static LUA51: StdlibLibrary<'static> = include!("stdlib_catalog/lua51.rs");
static LUA52: StdlibLibrary<'static> = include!("stdlib_catalog/lua52.rs");
static LUA53: StdlibLibrary<'static> = include!("stdlib_catalog/lua53.rs");
static LUA54: StdlibLibrary<'static> = include!("stdlib_catalog/lua54.rs");
static LUA55: StdlibLibrary<'static> = include!("stdlib_catalog/lua55.rs");
static LUAU: StdlibLibrary<'static> = include!("stdlib_catalog/luau.rs");
static LUAU_ROBLOX: StdlibLibrary<'static> = include!("stdlib_catalog/luau_roblox.rs");

#[must_use]
pub const fn library_for(target: LuaTarget) -> &'static StdlibLibrary<'static> {
    match target {
        LuaTarget::Lua51 => &LUA51,
        LuaTarget::Lua52 => &LUA52,
        LuaTarget::Lua53 => &LUA53,
        LuaTarget::Lua54 => &LUA54,
        LuaTarget::Lua55 => &LUA55,
        LuaTarget::Luau => &LUAU,
        LuaTarget::LuauRoblox => &LUAU_ROBLOX,
    }
}

pub(crate) const FUNCTION: StdlibFunction<'static> = StdlibFunction {
    signatures: &[],
    is_pure: false,
    is_must_use: false,
    deprecated: None,
    is_read_only: true,
    is_method: false,
    returns_shape: None,
};

pub(crate) const VALUE: StdlibValue<'static> =
    StdlibValue { is_read_only: true, deprecated: None, shape: None };

const ENUM_ITEM: StdlibEntry<'static> =
    StdlibEntry::Constant(&StdlibValue { shape: Some("EnumItem"), ..VALUE });

pub(crate) const fn required(kind: StdlibArgKind<'static>) -> StdlibParam<'static> {
    StdlibParam { kind, is_required: true, deprecated: None }
}

pub(crate) const fn optional(kind: StdlibArgKind<'static>) -> StdlibParam<'static> {
    StdlibParam { kind, is_required: false, deprecated: None }
}

const fn constant(value: &'static str) -> StdlibConstant<'static> {
    StdlibConstant { value, deprecated: None, is_common: false }
}

#[cfg(test)]
mod tests {
    use core::ptr;
    use std::fs;
    use std::path::PathBuf;

    use luck_allocator::Allocator;

    use super::*;
    use crate::stdlib::layer;
    use crate::stdlib_definition::StdlibDefinition;
    use crate::stdlib_model::StdlibMap;

    struct LibrarySpec {
        module: &'static str,
        /// Layered in this order, so a file can name what one before it
        /// declares.
        sources: &'static [&'static str],
        target: LuaTarget,
    }

    const LIBRARIES: [LibrarySpec; 7] = [
        LibrarySpec { module: "lua51", sources: &["lua51.toml"], target: LuaTarget::Lua51 },
        LibrarySpec { module: "lua52", sources: &["lua52.toml"], target: LuaTarget::Lua52 },
        LibrarySpec { module: "lua53", sources: &["lua53.toml"], target: LuaTarget::Lua53 },
        LibrarySpec { module: "lua54", sources: &["lua54.toml"], target: LuaTarget::Lua54 },
        LibrarySpec { module: "lua55", sources: &["lua55.toml"], target: LuaTarget::Lua55 },
        LibrarySpec { module: "luau", sources: &["luau.toml"], target: LuaTarget::Luau },
        LibrarySpec {
            module: "luau_roblox",
            sources: &["roblox_api.toml", "luau_roblox.toml", "roblox_enums.toml"],
            target: LuaTarget::LuauRoblox,
        },
    ];

    const INDENT: &str = "    ";

    /// Prints a library as the static expression `stdlib_catalog` includes.
    struct Emitter<'lib> {
        /// Every constant set, which a param that takes one names rather
        /// than repeats.
        sets: Vec<(&'lib str, &'lib [StdlibConstant<'lib>])>,
    }

    impl Emitter<'_> {
        /// `stdlib_map!(StdlibEntry<'static>, [...])`, opened on a line at
        /// `depth`.
        fn map(&self, entries: &StdlibMap<'_, StdlibEntry<'_>>, depth: usize) -> String {
            if entries.is_empty() {
                return "stdlib_map!(StdlibEntry<'static>, [])".to_owned();
            }
            let entry_depth = depth.saturating_add(1);
            let lines: Vec<String> = entries
                .iter()
                .map(|(name, value)| {
                    let indent = INDENT.repeat(entry_depth);
                    format!("{indent}({name:?}, {}),\n", self.entry(value, entry_depth))
                })
                .collect();
            format!(
                "\
stdlib_map!(StdlibEntry<'static>, [
{}{}])",
                lines.concat(),
                INDENT.repeat(depth)
            )
        }

        fn entry(&self, entry: &StdlibEntry<'_>, depth: usize) -> String {
            match entry {
                StdlibEntry::Function(function) => format!(
                    "StdlibEntry::Function(&{})",
                    literal("StdlibFunction", &self.function_fields(function), "FUNCTION")
                ),
                StdlibEntry::Constant(constant)
                    if constant.is_read_only
                        && constant.deprecated.is_none()
                        && constant.shape == Some("EnumItem") =>
                {
                    "ENUM_ITEM".to_owned()
                }
                StdlibEntry::Constant(constant) => {
                    format!("StdlibEntry::Constant(&{})", value(constant))
                }
                StdlibEntry::Property(property) => {
                    format!("StdlibEntry::Property(&{})", value(property))
                }
                // A namespace always sets its members, so it spells out every
                // field.
                StdlibEntry::Namespace(namespace) => format!(
                    "StdlibEntry::Namespace(&StdlibNamespace {{ members: {}, shape: {}, deprecated: \
                     {} }})",
                    self.map(&namespace.members, depth),
                    optional_name(namespace.shape),
                    namespace.deprecated.as_ref().map_or_else(|| "None".to_owned(), deprecation)
                ),
            }
        }

        fn function_fields(&self, function: &StdlibFunction<'_>) -> Vec<String> {
            let signatures: Vec<String> = function
                .signatures
                .iter()
                .map(|signature| {
                    let params: Vec<String> =
                        signature.params.iter().map(|param| self.param(param)).collect();
                    format!("StdlibSignature::new(&[{}])", params.join(", "))
                })
                .collect();
            let mut fields = vec![format!("signatures: &[{}]", signatures.join(", "))];
            if function.is_pure {
                fields.push("is_pure: true".to_owned());
            }
            if function.is_must_use {
                fields.push("is_must_use: true".to_owned());
            }
            if let Some(deprecated) = &function.deprecated {
                fields.push(format!("deprecated: {}", deprecation(deprecated)));
            }
            if !function.is_read_only {
                fields.push("is_read_only: false".to_owned());
            }
            if function.is_method {
                fields.push("is_method: true".to_owned());
            }
            if let Some(shape) = function.returns_shape {
                fields.push(format!("returns_shape: Some({shape:?})"));
            }
            fields
        }

        fn param(&self, param: &StdlibParam<'_>) -> String {
            let kind = self.arg_kind(&param.kind);
            param.deprecated.as_ref().map_or_else(
                || format!("{}({kind})", if param.is_required { "required" } else { "optional" }),
                |deprecated| {
                    format!(
                        "StdlibParam {{ kind: {kind}, is_required: {}, deprecated: {} }}",
                        param.is_required,
                        deprecation(deprecated)
                    )
                },
            )
        }

        fn arg_kind(&self, kind: &StdlibArgKind<'_>) -> String {
            match kind {
                StdlibArgKind::Any => "StdlibArgKind::Any".to_owned(),
                StdlibArgKind::Bool => "StdlibArgKind::Bool".to_owned(),
                StdlibArgKind::Number => "StdlibArgKind::Number".to_owned(),
                StdlibArgKind::String => "StdlibArgKind::String".to_owned(),
                StdlibArgKind::Function => "StdlibArgKind::Function".to_owned(),
                StdlibArgKind::Table => "StdlibArgKind::Table".to_owned(),
                StdlibArgKind::Display(label) => format!("StdlibArgKind::Display({label:?})"),
                StdlibArgKind::Constant(values) => {
                    let set = self.sets.iter().find(|(_, set)| ptr::eq(*set, *values));
                    set.map_or_else(
                        || {
                            let constants: Vec<String> =
                                values.iter().map(constant_literal).collect();
                            format!("StdlibArgKind::Constant(&[{}])", constants.join(", "))
                        },
                        |(name, _)| {
                            format!("StdlibArgKind::Constant(&{})", name.to_ascii_uppercase())
                        },
                    )
                }
                StdlibArgKind::Vararg => "StdlibArgKind::Vararg".to_owned(),
            }
        }

        fn shapes(&self, shapes: &StdlibMap<'_, StdlibShape<'_>>) -> String {
            if shapes.is_empty() {
                return "stdlib_map!(StdlibShape<'static>, [])".to_owned();
            }
            let lines: Vec<String> = shapes
                .iter()
                .map(|(name, shape)| {
                    format!(
                        "{INDENT}{INDENT}({name:?}, StdlibShape {{ members: {}, extends: {} }}),\n",
                        self.map(&shape.members, 2),
                        optional_name(shape.extends)
                    )
                })
                .collect();
            format!(
                "\
stdlib_map!(StdlibShape<'static>, [
{}{INDENT}])",
                lines.concat()
            )
        }

        fn constant_sets(&self) -> String {
            let entries: Vec<String> = self
                .sets
                .iter()
                .map(|(name, _)| format!("({name:?}, &{})", name.to_ascii_uppercase()))
                .collect();
            format!("stdlib_map!(&'static [StdlibConstant<'static>], [{}])", entries.join(", "))
        }

        fn set_static(name: &str, values: &[StdlibConstant<'_>]) -> String {
            let lines: Vec<String> = values
                .iter()
                .map(|value| format!("{INDENT}{},\n", constant_literal(value)))
                .collect();
            format!(
                "\
static {}: [StdlibConstant<'static>; {}] = [
{}];

",
                name.to_ascii_uppercase(),
                values.len(),
                lines.concat()
            )
        }
    }

    /// The struct literal setting `fields` on top of the catalog's `default`,
    /// or `default` itself when there are none.
    fn literal(type_name: &str, fields: &[String], default: &str) -> String {
        if fields.is_empty() {
            return default.to_owned();
        }
        format!("{type_name} {{ {}, ..{default} }}", fields.join(", "))
    }

    fn optional_name(name: Option<&str>) -> String {
        name.map_or_else(|| "None".to_owned(), |found| format!("Some({found:?})"))
    }

    fn constant_literal(constant: &StdlibConstant<'_>) -> String {
        let Some(deprecated) = &constant.deprecated else {
            if !constant.is_common {
                return format!("constant({:?})", constant.value);
            }
            return format!(
                "StdlibConstant {{ value: {:?}, deprecated: None, is_common: true }}",
                constant.value
            );
        };
        format!(
            "StdlibConstant {{ value: {:?}, deprecated: {}, is_common: {} }}",
            constant.value,
            deprecation(deprecated),
            constant.is_common
        )
    }

    fn deprecation(deprecation: &StdlibDeprecation<'_>) -> String {
        let replacement = deprecation.replacement.as_ref().map_or_else(
            || "None".to_owned(),
            |replacement| {
                format!(
                    "Some(StdlibReplacement {{ template: {:?}, is_exact: {} }})",
                    replacement.template, replacement.is_exact
                )
            },
        );
        format!(
            "Some(StdlibDeprecation {{ message: {:?}, replacement: {replacement} }})",
            deprecation.message
        )
    }

    fn value(value: &StdlibValue<'_>) -> String {
        let mut fields = Vec::new();
        if !value.is_read_only {
            fields.push("is_read_only: false".to_owned());
        }
        if let Some(deprecated) = &value.deprecated {
            fields.push(format!("deprecated: {}", deprecation(deprecated)));
        }
        if let Some(shape) = value.shape {
            fields.push(format!("shape: Some({shape:?})"));
        }
        literal("StdlibValue", &fields, "VALUE")
    }

    /// The library as the expression `stdlib_catalog` includes, which names
    /// everything in this module's scope. Constant sets are statics of a
    /// block around it.
    fn emit(spec: &LibrarySpec, library: &StdlibLibrary<'_>) -> String {
        let emitter = Emitter {
            sets: library.constant_sets.iter().map(|(name, set)| (name, *set)).collect(),
        };
        let statics: Vec<String> =
            emitter.sets.iter().map(|(name, values)| Emitter::set_static(name, values)).collect();
        let catalog = format!(
            "\
StdlibLibrary {{
{INDENT}target: LuaTarget::{:?},
{INDENT}globals: {},
{INDENT}shapes: {},
{INDENT}constant_sets: {},
}}",
            spec.target,
            emitter.map(&library.globals, 1),
            emitter.shapes(&library.shapes),
            emitter.constant_sets(),
        );
        let expression = if statics.is_empty() {
            catalog
        } else {
            format!("{{\n{}{catalog}\n}}", statics.concat())
        };
        let sources: Vec<String> =
            spec.sources.iter().map(|file| format!("stdlib_data/{file}")).collect();
        format!(
            "\
// Generated by `just stdlib` from {}. Never hand-edit it.

{expression}
",
            sources.join(", ")
        )
    }

    fn crate_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    fn generate(spec: &LibrarySpec) -> String {
        let allocator = Allocator::new();
        let mut library = StdlibLibrary::empty(spec.target);
        for file in spec.sources {
            let text = fs::read_to_string(crate_path(&format!("stdlib_data/{file}")))
                .expect("read the std data");
            assert!(text.is_ascii(), "stdlib_data/{file} isn't ASCII");
            let definition = StdlibDefinition::parse(&text)
                .unwrap_or_else(|diag| panic!("stdlib_data/{file}: {diag:?}"));
            library = layer(&allocator, &library, &definition)
                .unwrap_or_else(|diag| panic!("stdlib_data/{file}: {}", diag.message));
        }
        emit(spec, &library)
    }

    fn catalog_file(spec: &LibrarySpec) -> PathBuf {
        crate_path(&format!("src/stdlib_catalog/{}.rs", spec.module))
    }

    #[test]
    #[ignore = "rewrites src/stdlib_catalog from stdlib_data; run with --ignored to refresh"]
    fn regenerate_stdlib_catalog() {
        for spec in &LIBRARIES {
            fs::write(catalog_file(spec), generate(spec)).expect("write the catalog");
        }
    }

    #[test]
    fn stdlib_catalog_is_up_to_date() {
        for spec in &LIBRARIES {
            let committed = fs::read_to_string(catalog_file(spec)).expect("read the catalog");
            assert!(
                committed == generate(spec),
                "src/stdlib_catalog/{}.rs is stale; regenerate it with `just stdlib`",
                spec.module
            );
        }
    }
}
