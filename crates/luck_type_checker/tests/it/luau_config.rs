use std::num::NonZero;

use luck_type_checker::{CancellationToken, CheckError, luau_config_aliases};

use crate::common::{check, checker};

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

#[test]
fn a_config_luau_that_never_returns_runs_out_of_time() {
    assert_eq!(
        luau_config_aliases("while true do end\n"),
        Err("config:1: Typeinfer failed to complete in allotted time".to_owned())
    );
}

#[test]
fn a_config_luau_that_loops_in_a_coroutine_runs_out_of_time() {
    let source = "\
coroutine.wrap(function()
    while true do end
end)()
";

    assert_eq!(
        luau_config_aliases(source),
        Err("config:3: config:2: Typeinfer failed to complete in allotted time".to_owned())
    );
}

const MISMATCH: &str = "\
local count: number = \"one\"
return count
";

const STRICT: &str = "return { luau = { languagemode = \"strict\" } }\n";

#[test]
fn a_config_luau_stops_once_the_check_that_runs_it_is_cancelled() {
    let mut checker = checker(&[("main", MISMATCH)]);
    checker.host_mut().config_luau = Some("while true do end\n".to_owned());
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let cancelled = checker.check(&["main"], NonZero::<usize>::MIN, &cancellation);

    assert_eq!(cancelled, Err(CheckError::Cancelled));
    let errors = checker.host_mut().config_errors.get_mut();
    assert!(!errors.is_empty(), "the config ran");
    assert!(
        errors
            .iter()
            .all(|error| error == "invalid config: config:1: Analysis has been cancelled by user"),
        "{errors:?}"
    );

    // Luau can finish a module this small before it sees the cancellation,
    // under the config cut short, so the next check reads the config again
    // and checks the module under it.
    checker.host_mut().config_luau = Some(STRICT.to_owned());
    let diags = check(&mut checker, &["main"]).unwrap();
    assert_eq!(diags[0].len(), 1, "strict: {:?}", diags[0]);
}

#[test]
fn a_directory_s_config_luau_sets_the_mode_of_every_module_in_it() {
    let mut checker = checker(&[("first", MISMATCH), ("second", MISMATCH), ("third", MISMATCH)]);
    checker.host_mut().config_luau = Some(STRICT.to_owned());

    let diags = check(&mut checker, &["first", "second", "third"]).unwrap();

    assert!(diags.iter().all(|module| module.len() == 1), "each is strict: {diags:?}");
}

#[test]
fn a_directory_s_config_luau_runs_once_for_every_module_in_it() {
    let mut checker =
        checker(&[("first", "return 1\n"), ("second", "return 2\n"), ("third", "return 3\n")]);
    checker.host_mut().config_luau = Some("while true do end\n".to_owned());

    check(&mut checker, &["first", "second", "third"]).unwrap();

    assert_eq!(
        *checker.host_mut().config_errors.get_mut(),
        ["invalid config: config:1: Typeinfer failed to complete in allotted time"]
    );
}
