use luck_token::{LuaTarget, LuaVersion};
use serde::Deserialize as _;
use serde::de::IntoDeserializer as _;
use serde::de::value::Error;

const ALIASES: [(LuaTarget, &[&str]); 7] = [
    (LuaTarget::Lua51, &["lua51", "5.1", "51"]),
    (LuaTarget::Lua52, &["lua52", "5.2", "52"]),
    (LuaTarget::Lua53, &["lua53", "5.3", "53"]),
    (LuaTarget::Lua54, &["lua54", "5.4", "54"]),
    (LuaTarget::Lua55, &["lua55", "5.5", "55"]),
    (LuaTarget::Luau, &["luau"]),
    (LuaTarget::LuauRoblox, &["roblox", "roblox-luau", "luau-roblox"]),
];

#[test]
fn parses_every_alias_in_lowercase_and_uppercase() {
    for (target, aliases) in ALIASES {
        for alias in aliases {
            assert_eq!(alias.parse::<LuaTarget>(), Ok(target), "{alias}");
            let uppercase = alias.to_uppercase();
            assert_eq!(uppercase.parse::<LuaTarget>(), Ok(target), "{uppercase}");
        }
    }
}

#[test]
fn rejects_an_unknown_name() {
    let error = "invalid".parse::<LuaTarget>().unwrap_err();
    assert!(error.starts_with("invalid target `invalid`"), "{error}");
}

#[test]
fn display_round_trips_through_from_str() {
    for (target, _) in ALIASES {
        let displayed = target.to_string();
        assert_eq!(displayed.parse::<LuaTarget>(), Ok(target), "{displayed}");
    }
}

/// `luck.json` spells a dialect only as `FromStr` reads its lowercase
/// name, so a name it takes works on the command line too.
#[test]
fn deserializes_the_lowercase_names_from_str_reads() {
    for (target, aliases) in ALIASES {
        let name = aliases[0];
        let deserialized = LuaTarget::deserialize(name.into_deserializer());
        assert_eq!(deserialized, Ok::<_, Error>(target), "{name}");
    }
    let error = LuaTarget::deserialize("5.4".into_deserializer())
        .map_err(|e: Error| e.to_string())
        .unwrap_err();
    assert!(error.contains("unknown variant `5.4`"), "{error}");
}

#[test]
fn only_luau_on_roblox_is_roblox() {
    let roblox: Vec<LuaTarget> =
        LuaTarget::ALL.into_iter().filter(|target| target.is_roblox()).collect();
    assert_eq!(roblox, [LuaTarget::LuauRoblox]);
    assert_eq!(LuaTarget::LuauRoblox.version(), LuaVersion::Luau);
}

#[test]
fn a_version_names_its_standalone_target() {
    for version in LuaVersion::ALL {
        let target = LuaTarget::from(version);
        assert_eq!(target.version(), version);
        assert!(!target.is_roblox(), "{target}");
    }
}
