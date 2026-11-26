use luck_type_checker::luau_config_aliases;

#[test]
fn a_config_luau_s_aliases_come_back_by_lowercased_name() {
    let source = "\
local lib = \"./lib\"
return { luau = { languagemode = \"strict\", aliases = { Lib = lib } } }
";

    assert_eq!(luau_config_aliases(source), Ok(vec![("lib".to_owned(), "./lib".to_owned())]));
}

#[test]
fn a_config_luau_with_no_luau_table_declares_nothing() {
    assert_eq!(luau_config_aliases("return {}"), Ok(Vec::new()));
}

#[test]
fn a_config_luau_that_errors_comes_back_with_luau_s_message() {
    let error = luau_config_aliases("return require(\"./shared\")").unwrap_err();

    assert!(error.contains("attempt to call a nil value"), "{error}");
}

#[test]
fn a_config_luau_that_returns_no_table_comes_back_with_luau_s_message() {
    assert_eq!(
        luau_config_aliases("return 1"),
        Err("configuration did not return a table".to_owned())
    );
}
