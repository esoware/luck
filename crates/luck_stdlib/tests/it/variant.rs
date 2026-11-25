use luck_stdlib::Variant;
use serde::Deserialize as _;
use serde::de::IntoDeserializer as _;
use serde::de::value::Error;

const NAMES: [(Variant, &str); 2] =
    [(Variant::Standalone, "standalone"), (Variant::Roblox, "roblox")];

#[test]
fn parses_each_name_in_lowercase_and_uppercase() {
    for (variant, name) in NAMES {
        assert_eq!(name.parse::<Variant>(), Ok(variant), "{name}");
        let uppercase = name.to_uppercase();
        assert_eq!(uppercase.parse::<Variant>(), Ok(variant), "{uppercase}");
    }
}

#[test]
fn rejects_an_unknown_name() {
    for name in ["invalid", "bogus"] {
        let error = name.parse::<Variant>().unwrap_err();
        assert!(error.starts_with(&format!("invalid variant `{name}`")), "{error}");
    }
}

#[test]
fn display_round_trips_through_from_str() {
    for variant in Variant::ALL {
        let displayed = variant.to_string();
        assert_eq!(displayed.parse::<Variant>(), Ok(variant), "{displayed}");
    }
}

/// `luck.json` spells a variant only as `FromStr` reads its lowercase name,
/// so a name it takes works on the command line too.
#[test]
fn deserializes_the_lowercase_names_from_str_reads() {
    for (variant, name) in NAMES {
        let deserialized = Variant::deserialize(name.into_deserializer());
        assert_eq!(deserialized, Ok::<_, Error>(variant), "{name}");
    }
    let error = Variant::deserialize("bogus".into_deserializer())
        .map_err(|e: Error| e.to_string())
        .unwrap_err();
    assert!(error.contains("unknown variant `bogus`"), "{error}");
}

#[test]
fn only_roblox_is_roblox_and_standalone_is_the_default() {
    let roblox: Vec<Variant> =
        Variant::ALL.into_iter().filter(|variant| variant.is_roblox()).collect();
    assert_eq!(roblox, [Variant::Roblox]);
    assert_eq!(Variant::default(), Variant::Standalone);
}
