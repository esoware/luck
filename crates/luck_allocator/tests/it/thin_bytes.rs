use std::ptr;

use luck_allocator::{Allocator, ThinBytes};

use crate::common::AutoTraits;

#[test]
fn holds_bytes_in_three_words_of_four_bytes() {
    assert_eq!(size_of::<ThinBytes<'_>>(), 12);
    assert_eq!(align_of::<ThinBytes<'_>>(), 4);
    assert_eq!(size_of::<Option<ThinBytes<'_>>>(), 12);
}

#[test]
fn gives_back_the_bytes_it_was_made_from() {
    let allocator = Allocator::new();
    let bytes = allocator.alloc_slice_copy(b"a\xFFb");
    let thin = ThinBytes::new(bytes);
    assert!(ptr::eq(thin.as_bytes(), bytes));
    assert_eq!(thin.len(), 3);
    assert!(ThinBytes::new(b"").is_empty());
}

#[test]
fn compares_as_its_bytes_and_prints_them_quoted() {
    let thin = ThinBytes::from("a\"b");
    assert_eq!(thin, ThinBytes::new(b"a\"b"));
    assert_eq!(thin, &b"a\"b"[..]);
    assert_ne!(thin, ThinBytes::new(b"ab"));
    assert_eq!(format!("{thin:?}"), r#""a\"b""#);
    assert_eq!(format!("{:?}", ThinBytes::new(b"\xFF\n\xE9t\xC3\xA9")), "\"\\xFF\\n\\xE9t\u{e9}\"");
    let quoted = "'a\u{301}' \"\u{0}\"";
    assert_eq!(format!("{:?}", ThinBytes::from(quoted)), format!("{quoted:?}"));
}

#[test]
fn is_send_and_sync() {
    const {
        assert!(AutoTraits::<ThinBytes<'_>>::IS_SEND);
        assert!(AutoTraits::<ThinBytes<'_>>::IS_SYNC);
    }
}
