use crate::common::{check, checker};

const BROKEN: &str = "\
--!strict
local count: number = \"one\"
return count
";
const FIXED: &str = "\
--!strict
local count: number = 1
return count
";

#[test]
fn a_module_is_checked_again_only_once_invalidated() {
    let mut checker = checker(&[("main", BROKEN)]);
    assert_eq!(check(&mut checker, &["main"]).unwrap()[0].len(), 1);

    checker.host_mut().modules.insert("main".to_owned(), FIXED.to_owned());
    assert_eq!(check(&mut checker, &["main"]).unwrap()[0].len(), 1, "a check keeps what it read");

    checker.invalidate("main");
    assert_eq!(check(&mut checker, &["main"]).unwrap(), [[]]);
}

#[test]
fn invalidating_a_module_checks_its_requirers_again() {
    let main = "\
--!strict
local value = require(\"./value\")
local count: number = value
return count
";
    let mut checker = checker(&[("main", main), ("value", "return 1\n")]);
    assert_eq!(check(&mut checker, &["main"]).unwrap(), [[]]);

    checker.host_mut().modules.insert("value".to_owned(), "return \"one\"\n".to_owned());
    checker.invalidate("value");

    assert_eq!(check(&mut checker, &["main"]).unwrap()[0].len(), 1);
}
