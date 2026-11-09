//! The ignored `regenerate_roblox_api` test, which rewrites
//! `roblox_api.toml` and `roblox_enums.toml` from the live API dump, and
//! offline checks on the committed files.

use core::error::Error;
use core::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

const DUMP_URL: &str =
    "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/API-Dump.json";

/// Services of the settings providers (`settings()` and `UserSettings()`).
/// The dump tags them `Service`, but `game:GetService` can't reach them.
const SETTINGS_CHILDREN: &[&str] = &[
    "DebugSettings",
    "GameSettings",
    "LuaSettings",
    "NetworkSettings",
    "PhysicsSettings",
    "RenderSettings",
    "Studio",
    "UserGameSettings",
];

/// Curated frequently-used services, ranked first by completion.
const COMMON_SERVICES: &[&str] = &[
    "AnalyticsService",
    "AssetService",
    "AvatarEditorService",
    "BadgeService",
    "CaptureService",
    "ChangeHistoryService",
    "Chat",
    "CollectionService",
    "ContentProvider",
    "ContextActionService",
    "CoreGui",
    "DataStoreService",
    "Debris",
    "ExperienceNotificationService",
    "GamepadService",
    "GeometryService",
    "GroupService",
    "GuiService",
    "HapticService",
    "HttpService",
    "InsertService",
    "KeyframeSequenceProvider",
    "Lighting",
    "LocalizationService",
    "LogService",
    "MarketplaceService",
    "MaterialService",
    "MemoryStoreService",
    "MessagingService",
    "PathfindingService",
    "PhysicsService",
    "Players",
    "PolicyService",
    "ProximityPromptService",
    "ReplicatedFirst",
    "ReplicatedStorage",
    "RunService",
    "ScriptContext",
    "Selection",
    "ServerScriptService",
    "ServerStorage",
    "SocialService",
    "SoundService",
    "StarterGui",
    "StarterPack",
    "StarterPlayer",
    "Stats",
    "StudioService",
    "Teams",
    "TeleportService",
    "TestService",
    "TextChatService",
    "TextService",
    "TweenService",
    "UserInputService",
    "UserService",
    "VRService",
    "VoiceChatService",
    "Workspace",
];

fn generated_api_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/roblox_api.toml")
}

fn generated_enums_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stdlib_data/roblox_enums.toml")
}

fn value_line(name: &str, deprecation_message: Option<&str>, is_common: bool) -> String {
    let mut attributes: Vec<String> = Vec::new();
    if is_common {
        attributes.push("is_common = true".to_owned());
    }
    if let Some(message) = deprecation_message {
        attributes.push(format!("deprecated = {{ message = \"{message}\" }}"));
    }
    if attributes.is_empty() {
        format!("  \"{name}\",")
    } else {
        format!("  {{ value = \"{name}\", {} }},", attributes.join(", "))
    }
}

#[test]
#[ignore = "rewrites roblox_api.toml and roblox_enums.toml from the live API dump; run with --ignored"]
fn regenerate_roblox_api() {
    let body = ureq::get(DUMP_URL)
        .call()
        .expect("fetch Roblox API dump")
        .body_mut()
        .read_to_string()
        .expect("read API dump body");
    let dump: serde_json::Value = serde_json::from_str(&body).expect("parse API dump JSON");
    let classes = dump["Classes"].as_array().expect("Classes array");
    let mut rows: Vec<(String, bool, bool, bool)> = classes
        .iter()
        .map(|class| {
            let name = class["Name"].as_str().expect("class Name").to_owned();
            let tags: Vec<&str> = class["Tags"]
                .as_array()
                .map(|tags| tags.iter().filter_map(|tag| tag.as_str()).collect())
                .unwrap_or_default();
            (
                name,
                tags.contains(&"Service"),
                !tags.contains(&"NotCreatable"),
                tags.contains(&"Deprecated"),
            )
        })
        .collect();
    rows.sort();

    let mut services: Vec<String> = Vec::new();
    let mut creatable_classes: Vec<String> = Vec::new();
    let mut all_classes: Vec<String> = Vec::new();
    for (name, is_service, is_creatable, is_deprecated) in &rows {
        let class_deprecation_message = is_deprecated.then_some("deprecated Roblox class");
        if *is_service && !SETTINGS_CHILDREN.contains(&name.as_str()) {
            services.push(value_line(
                name,
                is_deprecated.then_some("deprecated Roblox service"),
                COMMON_SERVICES.contains(&name.as_str()),
            ));
        }
        if *is_creatable {
            creatable_classes.push(value_line(name, class_deprecation_message, false));
        }
        all_classes.push(value_line(name, class_deprecation_message, false));
    }
    for common_service in COMMON_SERVICES {
        assert!(
            rows.iter().any(|(name, is_service, ..)| name == common_service && *is_service),
            "curated common service {common_service} not in the dump"
        );
    }

    let mut generated = String::new();
    generated.push_str("# GENERATED FILE - do not hand-edit.\n");
    generated
        .push_str("# Regenerate: cargo test -p luck_semantic regenerate_roblox_api -- --ignored\n");
    generated.push_str(
        "# Source: Roblox API dump, MaximumADHD/Roblox-Client-Tracker (roblox branch).\n",
    );
    generated
        .push_str("# Spliced into the luau_roblox library; luau_roblox.toml params reference\n");
    generated.push_str("# these sets by name (see stdlib_load.rs for the constant_sets schema).\n");
    generated.push_str("# Settings-provider classes (settings() / UserSettings() children) are\n");
    generated.push_str(
        "# excluded from roblox_services: they are not reachable via game:GetService.\n\n",
    );
    generated.push_str("[constant_sets.roblox_services]\nvalues = [\n");
    generated.push_str(&services.join("\n"));
    generated.push_str("\n]\n\n[constant_sets.roblox_creatable_classes]\nvalues = [\n");
    generated.push_str(&creatable_classes.join("\n"));
    generated.push_str("\n]\n\n[constant_sets.roblox_all_classes]\nvalues = [\n");
    generated.push_str(&all_classes.join("\n"));
    generated.push_str("\n]\n");

    fs::write(generated_api_path(), generated).expect("write roblox_api.toml");
    write_enum_file(&dump).expect("write roblox_enums.toml");
}

/// Write `roblox_enums.toml`: every enum type and item in the dump, with
/// deprecation tags carried over. `splice_enums` in `stdlib_load.rs` splices
/// it in.
///
/// # Errors
///
/// Fails when the dump lacks an expected field or the file can't be
/// written.
fn write_enum_file(dump: &serde_json::Value) -> Result<(), Box<dyn Error>> {
    type EnumRow = (String, bool, Vec<(String, bool)>);
    let enums = dump["Enums"].as_array().ok_or("Enums array")?;
    let mut enum_types = enums
        .iter()
        .map(|enum_type| {
            let name = enum_type["Name"].as_str().ok_or("enum Name")?.to_owned();
            let is_deprecated = has_deprecated_tag(enum_type);
            let mut items = enum_type["Items"]
                .as_array()
                .ok_or("enum Items")?
                .iter()
                .map(|item| {
                    let item_name = item["Name"].as_str().ok_or("item Name")?.to_owned();
                    Ok((item_name, has_deprecated_tag(item)))
                })
                .collect::<Result<Vec<(String, bool)>, &str>>()?;
            items.sort();
            items.dedup_by(|current, previous| current.0 == previous.0);
            Ok((name, is_deprecated, items))
        })
        .collect::<Result<Vec<EnumRow>, &str>>()?;
    enum_types.sort();

    let mut generated = String::new();
    generated.push_str("# GENERATED FILE - do not hand-edit.\n");
    generated
        .push_str("# Regenerate: cargo test -p luck_semantic regenerate_roblox_api -- --ignored\n");
    generated.push_str(
        "# Source: Roblox API dump, MaximumADHD/Roblox-Client-Tracker (roblox branch).\n",
    );
    generated.push_str("# Spliced into the luau_roblox library: each [enums.<Type>] becomes an\n");
    generated.push_str("# Enum-shaped member namespace of the Enum global, each item an\n");
    generated.push_str("# EnumItem-shaped read-only constant (see stdlib_load.rs splice_enums).\n");
    for (name, is_deprecated, items) in &enum_types {
        writeln!(generated, "\n[enums.{name}]")?;
        if *is_deprecated {
            generated.push_str("deprecated = { message = \"deprecated Roblox enum\" }\n");
        }
        generated.push_str("items = [\n");
        for (item, is_item_deprecated) in items {
            if *is_item_deprecated {
                writeln!(
                    generated,
                    "  {{ value = \"{item}\", deprecated = {{ message = \"deprecated Roblox enum item\" }} }},"
                )?;
            } else {
                writeln!(generated, "  \"{item}\",")?;
            }
        }
        generated.push_str("]\n");
    }

    fs::write(generated_enums_path(), generated)?;
    Ok(())
}

fn has_deprecated_tag(node: &serde_json::Value) -> bool {
    node["Tags"]
        .as_array()
        .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some("Deprecated")))
}

#[test]
fn committed_roblox_enums_file_is_plausible() {
    let committed =
        fs::read_to_string(generated_enums_path()).expect("read committed roblox_enums.toml");
    assert!(committed.starts_with("# GENERATED FILE"));
    for sentinel in [
        "[enums.Material]",
        "[enums.KeyCode]",
        "[enums.EasingStyle]",
        "{ value = \"Parallel\", deprecated",
    ] {
        assert!(committed.contains(sentinel), "missing {sentinel}");
    }
}

/// Checks the committed `roblox_api.toml` offline: generated header and all
/// three sets. Every `roblox()` test already parses it.
#[test]
fn committed_roblox_api_file_is_plausible() {
    let committed =
        fs::read_to_string(generated_api_path()).expect("read committed roblox_api.toml");
    assert!(committed.starts_with("# GENERATED FILE"));
    for set in [
        "[constant_sets.roblox_services]",
        "[constant_sets.roblox_creatable_classes]",
        "[constant_sets.roblox_all_classes]",
    ] {
        assert!(committed.contains(set), "missing {set}");
    }
}
