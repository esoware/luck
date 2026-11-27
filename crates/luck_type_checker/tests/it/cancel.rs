use std::num::NonZero;
use std::thread;

use luck_type_checker::{CancellationToken, CheckError};

use crate::common::{check, checker};

const MISMATCH: &str = "\
--!strict
local count: number = \"one\"
return count
";

#[test]
fn a_token_cancelled_on_another_thread_cancels_the_check() {
    let mut checker = checker(&[("main", MISMATCH)]);
    let token = CancellationToken::new();
    let clone = token.clone();

    thread::scope(|scope| {
        scope.spawn(move || clone.cancel());
    });
    let cancelled = checker.check(&["main"], NonZero::<usize>::MIN, &token);

    assert_eq!(cancelled, Err(CheckError::Cancelled));
}

#[test]
fn the_next_check_finishes_what_a_cancelled_one_did_not() {
    let mut checker = checker(&[("main", MISMATCH)]);
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(checker.check(&["main"], NonZero::<usize>::MIN, &token), Err(CheckError::Cancelled));

    assert_eq!(check(&mut checker, &["main"]).unwrap()[0].len(), 1);
}
