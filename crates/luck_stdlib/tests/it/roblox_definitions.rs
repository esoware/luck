//! The ignored `regenerate_roblox_definitions` test, which writes
//! `roblox.luau` from the live API dump, `LuauTypes.d.luau`, and Roblox's
//! Creator Documentation, and offline checks of the committed file at each
//! security level.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::num::NonZero;
use std::path::PathBuf;

use flate2::read::GzDecoder;
use luck_allocator::Allocator;
use luck_stdlib::{SecurityLevel, Variant, definitions_for};
use luck_type_checker::{CancellationToken, Checker, Config, Host};
use saphyr::{LoadableYamlNode as _, SafelyIndex as _, Yaml};
use serde_json::Value;
use tar::Archive;

const DUMP_URL: &str =
    "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/API-Dump.json";

const LUAU_TYPES_URL: &str =
    "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/LuauTypes.d.luau";

const DOCS_URL: &str = "https://codeload.github.com/Roblox/creator-docs/tar.gz/refs/heads/main";

/// The levels a script can run at, weakest first, as the dump names them.
/// A member that needs any other, such as `RobloxSecurity`, is out of every
/// script's reach.
const LEVELS: [&str; 4] = ["None", "LocalUserSecurity", "PluginSecurity", "RobloxScriptSecurity"];

const HEADER: &str = "\
-- Roblox's engine API as a Luau definition file, which `just roblox-api`
-- generates from Roblox's API dump and LuauTypes.d.luau, as
-- https://github.com/MaximumADHD/Roblox-Client-Tracker extracts them from
-- Roblox Studio, and from the Roblox Creator Documentation,
-- https://github.com/Roblox/creator-docs.
--
-- The globals LuauTypes.d.luau's RobloxGlobals section declares, the ones
-- Studio gives a script, are its declarations, but for a global it types
-- `any` that the documentation types as a class, which Studio's reflection
-- types. Its other sections, such as TestEZ's and BehaviorScript's, aren't
-- in a script's environment, and are left out. The summaries, datatypes, and
-- other globals and libraries are adapted from the documentation,
-- (c) Roblox Corporation, under CC BY 4.0,
-- https://creativecommons.org/licenses/by/4.0/, as LICENSE-creator-docs beside
-- this file holds it.
--
-- It declares what a script can reach at RobloxScriptSecurity. A member that
-- needs a stronger level than None ends with a comment naming it, as
-- `-- PluginSecurity`, and a property a weaker level can only read names the
-- level that writes it, as `-- write: PluginSecurity`.

";

const KEYWORDS: [&str; 21] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

fn generated_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/roblox.luau")
}

fn fetch(url: &str) -> Vec<u8> {
    ureq::get(url)
        .call()
        .unwrap_or_else(|error| panic!("fetch {url}: {error}"))
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .unwrap_or_else(|error| panic!("read {url}: {error}"))
}

/// Each page of the docs' engine reference, by its path under it without
/// `.yaml`, as `classes/Part`.
fn fetch_docs() -> BTreeMap<String, Yaml<'static>> {
    let tarball = fetch(DOCS_URL);
    let mut pages = BTreeMap::new();
    let mut archive = Archive::new(GzDecoder::new(tarball.as_slice()));
    for entry in archive.entries().expect("read the docs tarball") {
        let mut entry = entry.expect("read a docs entry");
        let path = entry.path().expect("a docs path").to_string_lossy().into_owned();
        let Some((_, page)) = path.split_once("/content/en-us/reference/engine/") else {
            continue;
        };
        let Some(page) = page.strip_suffix(".yaml") else { continue };
        let mut text = String::new();
        entry.read_to_string(&mut text).expect("read a docs page");
        let mut documents = Yaml::load_from_str(&text).expect("a docs page is YAML");
        pages.insert(page.to_owned(), documents.swap_remove(0));
    }
    pages
}

fn list<'yaml>(node: &'yaml Yaml<'static>, key: &str) -> &'yaml [Yaml<'static>] {
    node.get(key).and_then(Yaml::as_vec).map_or(&[], Vec::as_slice)
}

fn text<'yaml>(node: &'yaml Yaml<'static>, key: &str) -> &'yaml str {
    node.get(key).and_then(Yaml::as_str).unwrap_or_default()
}

fn has_tag(node: &Yaml<'static>, tag: &str) -> bool {
    list(node, "tags").iter().any(|candidate| candidate.as_str() == Some(tag))
}

/// The member a docs name such as `Vector3.new` or `BasePart:GetMass` names.
fn member_name(name: &str) -> &str {
    name.rsplit(['.', ':']).next().unwrap_or(name)
}

/// A node's summary on one line, in ASCII, with each link to the engine
/// reference written as the docs show it, or `None` when it has none or
/// holds a character ASCII can't spell.
fn summary(node: &Yaml<'static>) -> Option<String> {
    let mut joined = String::new();
    for word in text(node, "summary").split_whitespace() {
        if !joined.is_empty() {
            joined.push(' ');
        }
        for character in word.chars() {
            match character {
                '\u{2011}' | '\u{2013}' | '\u{2014}' => joined.push('-'),
                '\u{2018}' | '\u{2019}' => joined.push('\''),
                '\u{201c}' | '\u{201d}' => joined.push('"'),
                '\u{2026}' => joined.push_str("..."),
                character if character.is_ascii() => joined.push(character),
                _ => return None,
            }
        }
    }
    if joined.is_empty() {
        return None;
    }
    let rendered = joined
        .split('`')
        .enumerate()
        .map(|(index, segment)| if index % 2 == 1 { reference(segment) } else { segment })
        .collect::<Vec<_>>()
        .join("`");
    Some(rendered)
}

/// The text the docs show for `code`, which links to the reference when it
/// starts with a kind, as `Class.BasePart.Size|Size` shows `Size`.
fn reference(code: &str) -> &str {
    const KINDS: [&str; 5] =
        ["Class.", "Datatype.", "Library.", "Global.LuaGlobals.", "Global.RobloxGlobals."];
    if !code.starts_with("Enum.") && !KINDS.iter().any(|kind| code.starts_with(kind)) {
        return code;
    }
    if let Some((_, shown)) = code.split_once('|') {
        return shown;
    }
    KINDS.iter().find_map(|kind| code.strip_prefix(kind)).unwrap_or(code)
}

fn write_summary(out: &mut String, indent: &str, summary: Option<&str>) {
    if let Some(summary) = summary {
        writeln!(out, "{indent}-- {summary}").expect("write to a string");
    }
}

fn is_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !KEYWORDS.contains(&name)
}

/// `name` as the key of a property or a table field.
fn key(name: &str) -> String {
    if is_identifier(name) {
        name.to_owned()
    } else {
        format!("[\"{}\"]", name.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// `name` as a parameter's, which a keyword or a space can't be.
fn param_name(name: &str) -> String {
    if KEYWORDS.contains(&name) { format!("{name}_") } else { name.replace(' ', "_") }
}

fn optional(ty: &str) -> String {
    if ty == "any" || ty == "nil" || ty.ends_with('?') {
        ty.to_owned()
    } else if ty.contains(" | ") {
        format!("({ty})?")
    } else {
        format!("{ty}?")
    }
}

struct Param {
    name: String,
    /// Its type, which starts with `...` for the rest of the arguments.
    ty: String,
}

/// `params` as a declaration lists them, after `self` for a method, or as a
/// function type does.
fn param_list(params: &[Param], is_method: bool, is_type: bool) -> String {
    let mut listed: Vec<String> = Vec::new();
    if is_method {
        listed.push("self".to_owned());
    }
    for (index, param) in params.iter().enumerate() {
        let rest = param.ty.strip_prefix("...");
        listed.push(match rest {
            Some(rest) if index + 1 == params.len() && is_type => format!("...{rest}"),
            Some(rest) if index + 1 == params.len() => format!("...: {rest}"),
            Some(_) => format!("{}: any", param.name),
            None => format!("{}: {}", param.name, param.ty),
        });
    }
    format!("({})", listed.join(", "))
}

/// The types a function returns, as a declaration's `:` or a function type's
/// `->` takes them.
fn return_list(types: &[String]) -> String {
    match types {
        [] => "()".to_owned(),
        [only] if only == "nil" || only == "()" => "()".to_owned(),
        [only] if only.contains(" | ") => format!("({only})"),
        [only] => only.clone(),
        many => format!("({})", many.join(", ")),
    }
}

/// The names the file declares types for.
struct Names {
    /// Roblox's classes and datatypes.
    externs: BTreeSet<String>,
    enums: BTreeSet<String>,
}

impl Names {
    /// The Luau type of the type Roblox names `name`, in the dump or the
    /// docs, which `any` stands for when nothing declares it.
    fn named(&self, name: &str) -> String {
        if let Some(inner) = name.strip_suffix('?') {
            return optional(&self.named(inner));
        }
        let ty = match name {
            "bool" | "boolean" => "boolean",
            "int" | "int64" | "float" | "double" | "number" => "number",
            "string" | "ContentId" | "BinaryString" | "ProtectedString" | "SharedString"
            | "QDir" | "QFont" => "string",
            "null" | "nil" | "void" => "nil",
            "Variant" | "any" => "any",
            "thread" | "buffer" | "vector" | "()" => name,
            "Array" | "table" => "{ any }",
            "Dictionary" => "{ [string]: any }",
            "Map" => "{ [any]: any }",
            "Function" | "function" => "((...any) -> ...any)",
            "Instances" | "Objects" => "{ Instance }",
            "CoordinateFrame" => "CFrame",
            "OptionalCoordinateFrame" => "CFrame?",
            "Tuple" => "...any",
            _ if self.externs.contains(name) => name,
            _ => {
                let enum_name = name.strip_prefix("Enum.").unwrap_or(name);
                if self.enums.contains(enum_name) {
                    return format!("Enum{enum_name}");
                }
                "any"
            }
        };
        ty.to_owned()
    }

    /// The Luau type of a type the docs write, such as `Array<Instance>` or
    /// `function | thread`.
    fn docs(&self, ty: &str) -> String {
        let ty = ty.trim();
        if ty.contains('|') {
            return ty.split('|').map(|part| self.docs(part)).collect::<Vec<_>>().join(" | ");
        }
        if let Some(inner) = ty.strip_suffix('?') {
            return optional(&self.docs(inner));
        }
        if let Some(inner) = ty
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
            .or_else(|| ty.strip_prefix("Array<").and_then(|rest| rest.strip_suffix('>')))
        {
            return format!("{{ {} }}", self.docs(inner));
        }
        if let Some(inner) = ty.strip_prefix("Tuple<").and_then(|rest| rest.strip_suffix('>')) {
            return format!("...{}", self.docs(inner));
        }
        self.named(ty)
    }

    /// The Luau type of a type the dump gives, `{ Category, Name }`.
    fn dump(&self, ty: &Value) -> String {
        let name = ty["Name"].as_str().expect("a type's Name");
        if ty["Category"] == "Enum" {
            let (base, is_optional) =
                name.strip_suffix('?').map_or((name, false), |base| (base, true));
            let enum_type = format!("Enum{base}");
            return if is_optional { optional(&enum_type) } else { enum_type };
        }
        self.named(name)
    }

    fn dump_params(&self, member: &Value) -> Vec<Param> {
        member["Parameters"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|param| {
                let ty = self.dump(&param["Type"]);
                let ty = if param.get("Default").is_some() { optional(&ty) } else { ty };
                let name = param["Name"].as_str().expect("a parameter's Name");
                Param { name: param_name(name), ty }
            })
            .collect()
    }

    fn dump_returns(&self, member: &Value) -> String {
        let returned = &member["ReturnType"];
        let types: Vec<String> = match returned.as_array() {
            Some(types) => types.iter().map(|ty| self.dump(ty)).collect(),
            None => vec![self.dump(returned)],
        };
        return_list(&types)
    }

    fn docs_params(&self, function: &Yaml<'static>) -> Vec<Param> {
        list(function, "parameters")
            .iter()
            .map(|param| {
                let ty = self.docs(text(param, "type"));
                let name = text(param, "name");
                if name == "..." {
                    let rest = ty.strip_prefix("...").unwrap_or(&ty);
                    return Param { name: String::new(), ty: format!("...{rest}") };
                }
                let has_default = param.get("default").is_some_and(|default| !default.is_null());
                Param { name: param_name(name), ty: if has_default { optional(&ty) } else { ty } }
            })
            .collect()
    }

    /// What `function` returns, which for a constructor, which lists none, is
    /// the datatype it `constructs`.
    fn docs_returns(&self, function: &Yaml<'static>, constructs: Option<&str>) -> String {
        let mut types: Vec<String> =
            list(function, "returns").iter().map(|ty| self.docs(text(ty, "type"))).collect();
        if types.is_empty() {
            types.extend(constructs.map(str::to_owned));
        }
        return_list(&types)
    }

    /// Each function of `page`'s `key`, as table fields declare them: its
    /// name, its summary, and its type, an intersection of its overloads.
    fn docs_functions(
        &self,
        page: &Yaml<'static>,
        key: &str,
        constructs: Option<&str>,
    ) -> Vec<Field> {
        let mut fields: Vec<Field> = Vec::new();
        for function in list(page, key) {
            let name = member_name(text(function, "name"));
            let signature = format!(
                "{} -> {}",
                param_list(&self.docs_params(function), false, true),
                self.docs_returns(function, constructs)
            );
            match fields.iter_mut().find(|field| field.name == name) {
                Some(field) => field.overloads.push(signature),
                None => fields.push(Field {
                    name: name.to_owned(),
                    summary: summary(function),
                    overloads: vec![signature],
                    is_deprecated: has_tag(function, "Deprecated"),
                }),
            }
        }
        fields
    }

    fn docs_properties(&self, page: &Yaml<'static>, key: &str) -> Vec<Field> {
        list(page, key)
            .iter()
            .map(|property| Field {
                name: member_name(text(property, "name")).to_owned(),
                summary: summary(property),
                overloads: vec![self.docs(text(property, "type"))],
                is_deprecated: false,
            })
            .collect()
    }
}

/// A field of a table type a global has.
struct Field {
    name: String,
    summary: Option<String>,
    /// Its type, or for a function, the type of each overload.
    overloads: Vec<String>,
    is_deprecated: bool,
}

impl Field {
    fn ty(&self) -> String {
        match self.overloads.as_slice() {
            [only] if self.is_deprecated => format!("@deprecated {only}"),
            [only] => only.clone(),
            many => many.iter().map(|ty| format!("({ty})")).collect::<Vec<_>>().join("\n\t\t& "),
        }
    }
}

/// A table type of `fields`, one to a line, after `indent`.
fn table(fields: &[Field], indent: &str) -> String {
    let mut out = String::from("{\n");
    for field in fields {
        write_summary(&mut out, &format!("{indent}\t"), field.summary.as_deref());
        writeln!(out, "{indent}\t{}: {},", key(&field.name), field.ty())
            .expect("write to a string");
    }
    out.push_str(indent);
    out.push('}');
    out
}

/// The level a script needs for `name`, or `None` when no script can reach
/// it.
fn level(name: &Value) -> Option<usize> {
    LEVELS.iter().position(|level| name == level)
}

fn dump_tags(node: &Value) -> Vec<&str> {
    node["Tags"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

/// `member`'s line in its class, with the comment that names the levels it
/// needs, or `None` when no script can reach it.
fn member_line(names: &Names, member: &Value) -> Option<String> {
    let tags = dump_tags(member);
    if tags.contains(&"NotScriptable") {
        return None;
    }
    let name = member["Name"].as_str().expect("a member's Name");
    let security = &member["Security"];
    let (read, write) = match security.get("Read") {
        Some(read) => (read, &security["Write"]),
        None => (security, security),
    };
    // Luau takes `read` and `write` only before a name, so a property whose
    // name isn't one, such as one of Studio's settings, can be both.
    let (read_keyword, write_keyword) =
        if is_identifier(name) { ("read ", "write ") } else { ("", "") };
    let mut levels: Vec<String> = Vec::new();
    let line = match member["MemberType"].as_str() {
        Some("Property") => {
            let ty = names.dump(&member["ValueType"]);
            let (access, write_level) = (level(read), level(write));
            if tags.contains(&"WriteOnly") {
                let write_access = write_level?;
                levels.extend((write_access > 0).then(|| LEVELS[write_access].to_owned()));
                format!("{write_keyword}{}: {ty}", key(name))
            } else {
                let access = access?;
                levels.extend((access > 0).then(|| LEVELS[access].to_owned()));
                match write_level.filter(|_| !tags.contains(&"ReadOnly")) {
                    None => format!("{read_keyword}{}: {ty}", key(name)),
                    Some(write_level) => {
                        if write_level > access && !read_keyword.is_empty() {
                            levels.push(format!("write: {}", LEVELS[write_level]));
                        }
                        format!("{}: {ty}", key(name))
                    }
                }
            }
        }
        Some(kind) => {
            let access = level(read)?;
            levels.extend((access > 0).then(|| LEVELS[access].to_owned()));
            match kind {
                "Function" => {
                    let deprecated = if tags.contains(&"Deprecated") { "@deprecated " } else { "" };
                    format!(
                        "{deprecated}function {name}{}: {}",
                        param_list(&names.dump_params(member), true, false),
                        names.dump_returns(member)
                    )
                }
                "Event" => format!("{read_keyword}{}: RBXScriptSignal", key(name)),
                "Callback" => format!(
                    "{write_keyword}{}: {} -> {}",
                    key(name),
                    param_list(&names.dump_params(member), false, true),
                    names.dump_returns(member)
                ),
                _ => panic!("unknown member type {kind}"),
            }
        }
        None => panic!("a member without a MemberType"),
    };
    Some(if levels.is_empty() { line } else { format!("{line} -- {}", levels.join(", ")) })
}

/// Each declaration of the `RobloxGlobals` section of `LuauTypes.d.luau`, the
/// globals Roblox Studio gives a script, with the name it declares. A
/// `declare class`, which Luau now reads as a global named `class`, becomes
/// the `declare extern type` it means.
fn studio_globals(luau_types: &str) -> Vec<(String, String)> {
    let (_, section) = luau_types
        .split_once("-- SECTION BEGIN: RobloxGlobals\n")
        .expect("LuauTypes.d.luau has a RobloxGlobals section");
    let (section, _) = section.split_once("-- SECTION END: RobloxGlobals").expect("it ends");
    let mut declarations: Vec<(String, String)> = Vec::new();
    for line in section.lines().filter(|line| !line.trim().is_empty()) {
        let line = match line.strip_prefix("declare class ") {
            Some(header) => format!("declare extern type {header} with"),
            None => line.to_owned(),
        };
        if let Some(declared) = line.strip_prefix("declare ") {
            let declared = declared
                .strip_prefix("function ")
                .or_else(|| declared.strip_prefix("extern type "))
                .unwrap_or(declared);
            let name = declared
                .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .next()
                .unwrap_or_default();
            declarations.push((name.to_owned(), String::new()));
        }
        let (_, text) = declarations.last_mut().expect("a declaration starts the section");
        text.push_str(&line);
        text.push('\n');
    }
    declarations
}

/// Which of `candidates`, each a global or a field of one, as `debug.info`,
/// Luau's built-in definitions don't declare.
fn missing_from_luau(candidates: &[String]) -> BTreeSet<String> {
    struct Probe(String);

    impl Host for Probe {
        fn source(&self, module: &str) -> Option<Vec<u8>> {
            (module == "probe").then(|| self.0.clone().into_bytes())
        }

        fn resolve(&self, _: &str, _: &str) -> Option<String> {
            None
        }

        fn config(&self, _: &str, _: &mut Config<'_>) {}
    }

    let mut probe = String::from("--!strict\n");
    for candidate in candidates {
        writeln!(probe, "local _ = {candidate}").expect("write to a string");
    }
    let mut checker = Checker::new(Probe(probe.clone()));
    let diags = checker
        .check(&["probe"], NonZero::<usize>::MIN, &CancellationToken::new())
        .expect("the probe checks");
    diags[0]
        .iter()
        .filter_map(|diag| {
            let start = diag.span?.start as usize;
            let line = probe[..start].matches('\n').count();
            candidates.get(line - 1).cloned()
        })
        .collect()
}

#[test]
#[ignore = "rewrites roblox.luau from the live API dump, LuauTypes.d.luau, and Creator Documentation; run with --ignored"]
fn regenerate_roblox_definitions() {
    let dump: Value = serde_json::from_slice(&fetch(DUMP_URL)).expect("the API dump is JSON");
    let luau_types = String::from_utf8(fetch(LUAU_TYPES_URL)).expect("LuauTypes.d.luau is UTF-8");
    let docs = fetch_docs();
    let docs_page = |kind: &str, name: &str| docs.get(&format!("{kind}/{name}"));

    let classes: BTreeMap<&str, &Value> = dump["Classes"]
        .as_array()
        .expect("Classes")
        .iter()
        .map(|class| (class["Name"].as_str().expect("a class Name"), class))
        .collect();
    let enums: BTreeMap<&str, &Value> = dump["Enums"]
        .as_array()
        .expect("Enums")
        .iter()
        .map(|dump_enum| (dump_enum["Name"].as_str().expect("an enum Name"), dump_enum))
        .collect();
    let datatypes: Vec<&str> =
        docs.keys().filter_map(|page| page.strip_prefix("datatypes/")).collect();
    let names = Names {
        externs: classes.keys().chain(&datatypes).map(|name| (*name).to_owned()).collect(),
        enums: enums.keys().map(|name| (*name).to_owned()).collect(),
    };
    for name in enums.keys() {
        assert!(!names.externs.contains(&format!("Enum{name}")), "Enum{name} names a type too");
    }

    let mut out = String::from(HEADER);
    for &name in datatypes.iter().filter(|name| !classes.contains_key(*name)) {
        let page = docs_page("datatypes", name).expect("a datatype's page");
        let constants: BTreeSet<&str> = list(page, "constants")
            .iter()
            .map(|constant| member_name(text(constant, "name")))
            .collect();
        write_summary(&mut out, "", summary(page).as_deref());
        writeln!(out, "declare extern type {name} with").expect("write to a string");
        for property in list(page, "properties") {
            let member = member_name(text(property, "name"));
            if constants.contains(member) {
                continue;
            }
            write_summary(&mut out, "\t", summary(property).as_deref());
            writeln!(out, "\t{}: {}", key(member), names.docs(text(property, "type")))
                .expect("write to a string");
        }
        for method in list(page, "methods") {
            write_summary(&mut out, "\t", summary(method).as_deref());
            let deprecated = if has_tag(method, "Deprecated") { "@deprecated " } else { "" };
            writeln!(
                out,
                "\t{deprecated}function {}{}: {}",
                member_name(text(method, "name")),
                param_list(&names.docs_params(method), true, false),
                names.docs_returns(method, None)
            )
            .expect("write to a string");
        }
        // The `Enum` global is an `Enums`, which holds each enum.
        if name == "Enums" {
            for (enum_name, dump_enum) in &enums {
                let enum_page = docs_page("enums", enum_name);
                let item_summaries: BTreeMap<&str, String> = enum_page
                    .map(|found| list(found, "items"))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|docs_item| Some((text(docs_item, "name"), summary(docs_item)?)))
                    .collect();
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                let items: Vec<Field> = dump_enum["Items"]
                    .as_array()
                    .expect("an enum's Items")
                    .iter()
                    .filter_map(|dump_item| dump_item["Name"].as_str())
                    .filter(|item| seen.insert(item))
                    .map(|item| Field {
                        name: item.to_owned(),
                        summary: item_summaries.get(item).cloned(),
                        overloads: vec![format!("Enum{enum_name}")],
                        is_deprecated: false,
                    })
                    .collect();
                write_summary(&mut out, "\t", enum_page.and_then(summary).as_deref());
                writeln!(out, "\t{}: Enum & {}", key(enum_name), table(&items, "\t"))
                    .expect("write to a string");
            }
        }
        let mut metamethods: Vec<String> = Vec::new();
        for operation in list(page, "math_operations") {
            let metamethod = match text(operation, "operation") {
                "+" => "__add",
                "-" => "__sub",
                "*" => "__mul",
                "/" => "__div",
                "//" => "__idiv",
                "%" => "__mod",
                "^" => "__pow",
                ".." => "__concat",
                other => panic!("unknown operation {other} of {name}"),
            };
            // Luau's checker calls the metamethod of the right operand, when
            // only it has one, with the operands swapped.
            let (left, right) = (text(operation, "type_a"), text(operation, "type_b"));
            let other = if left == name { right } else { left };
            let line = format!(
                "\tfunction {metamethod}(self, other: {}): {}",
                names.docs(other),
                names.docs(text(operation, "return_type"))
            );
            if !metamethods.contains(&line) {
                write_summary(&mut out, "\t", summary(operation).as_deref());
                writeln!(out, "{line}").expect("write to a string");
                metamethods.push(line);
            }
        }
        out.push_str("end\n\n");
    }

    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    let mut order: Vec<&str> = Vec::new();
    for &name in classes.keys() {
        let mut chain = vec![name];
        while let Some(superclass) =
            classes.get(chain[chain.len() - 1]).and_then(|class| class["Superclass"].as_str())
        {
            if !classes.contains_key(superclass) {
                break;
            }
            chain.push(superclass);
        }
        for &class in chain.iter().rev() {
            if emitted.insert(class) {
                order.push(class);
            }
        }
    }
    for name in order {
        let class = classes[name];
        let class_page = docs_page("classes", name);
        let mut summaries: BTreeMap<&str, String> = BTreeMap::new();
        for kind in ["properties", "methods", "events", "callbacks"] {
            for member in class_page.map(|page| list(page, kind)).unwrap_or_default() {
                if let Some(member_summary) = summary(member) {
                    summaries.insert(member_name(text(member, "name")), member_summary);
                }
            }
        }
        write_summary(&mut out, "", class_page.and_then(summary).as_deref());
        let extends = class["Superclass"]
            .as_str()
            .filter(|superclass| classes.contains_key(superclass))
            .map(|superclass| format!(" extends {superclass}"))
            .unwrap_or_default();
        writeln!(out, "declare extern type {name}{extends} with").expect("write to a string");
        for member in class["Members"].as_array().expect("Members") {
            let Some(line) = member_line(&names, member) else { continue };
            let member_name = member["Name"].as_str().expect("a member's Name");
            write_summary(&mut out, "\t", summaries.get(member_name).map(String::as_str));
            writeln!(out, "\t{line}").expect("write to a string");
        }
        out.push_str("end\n\n");
    }

    for name in enums.keys() {
        writeln!(out, "declare extern type Enum{name} extends EnumItem with end")
            .expect("write to a string");
    }
    out.push('\n');

    for &name in &datatypes {
        let page = docs_page("datatypes", name).expect("a datatype's page");
        let mut fields = names.docs_functions(page, "constructors", Some(name));
        fields.extend(names.docs_properties(page, "constants"));
        fields.extend(names.docs_functions(page, "functions", None));
        if !fields.is_empty() {
            writeln!(out, "declare {name}: {}\n", table(&fields, "")).expect("write to a string");
        }
    }

    let globals = ["globals/RobloxGlobals", "globals/LuaGlobals"].map(|name| &docs[name]);
    let libraries: Vec<(&str, &Yaml<'static>)> = docs
        .iter()
        .filter_map(|(name, page)| Some((name.strip_prefix("libraries/")?, page)))
        .collect();
    let mut candidates: Vec<String> = Vec::new();
    for page in globals {
        for kind in ["properties", "functions"] {
            candidates.extend(list(page, kind).iter().map(|item| text(item, "name").to_owned()));
        }
    }
    for (name, page) in &libraries {
        candidates.push((*name).to_owned());
        for kind in ["properties", "functions"] {
            candidates.extend(list(page, kind).iter().map(|item| text(item, "name").to_owned()));
        }
    }
    let missing = missing_from_luau(&candidates);

    let mut global_summaries: BTreeMap<String, String> = BTreeMap::new();
    let mut global_classes: BTreeMap<&str, &str> = BTreeMap::new();
    for page in globals.iter().copied().chain(libraries.iter().map(|(_, page)| *page)) {
        for kind in ["properties", "functions"] {
            for item in list(page, kind) {
                if let Some(item_summary) = summary(item) {
                    global_summaries.insert(text(item, "name").to_owned(), item_summary);
                }
                let ty = text(item, "type");
                if classes.contains_key(ty) {
                    global_classes.insert(text(item, "name"), ty);
                }
            }
        }
    }
    for (name, page) in &libraries {
        global_summaries
            .extend(summary(page).map(|page_summary| ((*name).to_owned(), page_summary)));
    }
    let studio = studio_globals(&luau_types);
    for (name, declaration) in &studio {
        write_summary(&mut out, "", global_summaries.get(name).map(String::as_str));
        if let Some(class) = global_classes.get(name.as_str())
            && *declaration == format!("declare {name}: any\n")
        {
            writeln!(out, "declare {name}: {class}\n").expect("write to a string");
            continue;
        }
        for line in declaration.lines() {
            let field = line.trim_start();
            let indent = &line[..line.len() - field.len()];
            let field_name = field.split(':').next().unwrap_or_default();
            if !indent.is_empty() {
                let field_summary = global_summaries.get(&format!("{name}.{field_name}"));
                write_summary(&mut out, indent, field_summary.map(String::as_str));
            }
            writeln!(out, "{line}").expect("write to a string");
        }
        out.push('\n');
    }
    let from_studio: BTreeSet<&str> = studio.iter().map(|(name, _)| name.as_str()).collect();

    for page in globals {
        for property in names.docs_properties(page, "properties") {
            if !missing.contains(&property.name) || from_studio.contains(property.name.as_str()) {
                continue;
            }
            write_summary(&mut out, "", property.summary.as_deref());
            writeln!(out, "declare {}: {}\n", key(&property.name), property.ty())
                .expect("write to a string");
        }
        for function in list(page, "functions") {
            let name = text(function, "name");
            if !missing.contains(name) || from_studio.contains(name) {
                continue;
            }
            write_summary(&mut out, "", summary(function).as_deref());
            let deprecated = if has_tag(function, "Deprecated") { "@deprecated " } else { "" };
            writeln!(
                out,
                "{deprecated}declare function {name}{}: {}\n",
                param_list(&names.docs_params(function), false, false),
                names.docs_returns(function, None)
            )
            .expect("write to a string");
        }
    }
    for (name, page) in libraries.iter().filter(|(name, _)| !from_studio.contains(name)) {
        let mut fields = names.docs_properties(page, "properties");
        fields.extend(names.docs_functions(page, "functions", None));
        if missing.contains(*name) {
            write_summary(&mut out, "", summary(page).as_deref());
            writeln!(out, "declare {name}: {}\n", table(&fields, "")).expect("write to a string");
        } else {
            fields.retain(|field| missing.contains(&format!("{name}.{}", field.name)));
            if !fields.is_empty() {
                writeln!(out, "declare {name}: typeof({name}) & {}\n", table(&fields, ""))
                    .expect("write to a string");
            }
        }
    }

    let trimmed = out.trim_end().to_owned() + "\n";
    fs::write(generated_path(), trimmed).expect("write roblox.luau");
}

/// Each security level's definitions, which every test below reads.
fn every_level() -> Vec<(SecurityLevel, String)> {
    SecurityLevel::ALL
        .iter()
        .map(|&level| {
            let text = definitions_for(Variant::Roblox, level).expect("Roblox has definitions");
            (level, text.into_owned())
        })
        .collect()
}

#[test]
fn standalone_luau_has_no_definitions_beyond_luaus_own() {
    for level in SecurityLevel::ALL {
        assert!(definitions_for(Variant::Standalone, level).is_none(), "{level:?}");
    }
}

#[test]
fn the_definitions_parse_at_every_level() {
    for (level, text) in every_level() {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse_definitions(&allocator, text.as_bytes());
        assert!(parsed.errors.is_empty(), "{level:?}: {:?}", parsed.errors);
    }
}

#[test]
fn the_definitions_load_into_a_checker_at_every_level() {
    struct NoModules;

    impl Host for NoModules {
        fn source(&self, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn resolve(&self, _: &str, _: &str) -> Option<String> {
            None
        }

        fn config(&self, _: &str, _: &mut Config<'_>) {}
    }

    for (level, text) in every_level() {
        let mut checker = Checker::new(NoModules);
        let loaded = checker.load_definitions("@roblox", &text);
        assert!(loaded.is_ok(), "{level:?}: {loaded:?}");
    }
}

/// The member lines of the extern type `name` in `text`.
fn extern_type<'text>(text: &'text str, name: &str) -> Vec<&'text str> {
    let header = format!("declare extern type {name} ");
    let start = text.find(&header).unwrap_or_else(|| panic!("{name} is declared"));
    text[start..].lines().skip(1).take_while(|line| *line != "end").collect()
}

fn declares(members: &[&str], member: &str) -> bool {
    members.iter().any(|line| line.trim_start().starts_with(member))
}

/// `GetDebugId` needs `PluginSecurity`.
#[test]
fn a_member_is_declared_from_the_level_it_needs_up() {
    for (level, text) in every_level() {
        let instance = extern_type(&text, "Instance");
        assert_eq!(
            declares(&instance, "function GetDebugId("),
            level >= SecurityLevel::PluginSecurity,
            "{level:?}"
        );
    }
}

#[test]
fn a_member_left_out_takes_its_summary_with_it() {
    let levels = every_level();
    let full = extern_type(&levels[3].1, "Instance");
    let index = full.iter().position(|line| line.contains("function GetDebugId(")).unwrap();
    let summary = full[index - 1];
    assert!(summary.trim_start().starts_with("-- "), "{summary}");

    assert!(!extern_type(&levels[0].1, "Instance").contains(&summary));
}

/// Any script reads `HttpService.HttpEnabled`, but writing it needs
/// `LocalUserSecurity`.
#[test]
fn a_property_a_level_can_only_read_is_read_only_there() {
    for (level, text) in every_level() {
        let http = extern_type(&text, "HttpService");
        let member = if level >= SecurityLevel::LocalUserSecurity {
            "HttpEnabled: boolean"
        } else {
            "read HttpEnabled: boolean"
        };
        assert!(declares(&http, member), "{level:?}: {http:?}");
    }
}

/// `Player:GetGameSessionID` needs `RobloxSecurity`, and writing
/// `Player.AgeChecked` does.
#[test]
fn no_level_reaches_what_needs_roblox_security() {
    for (level, text) in every_level() {
        let player = extern_type(&text, "Player");
        assert!(!declares(&player, "function GetGameSessionID("), "{level:?}");
        assert!(declares(&player, "read AgeChecked: "), "{level:?}");
    }
}
