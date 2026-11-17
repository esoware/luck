use std::{
    cell::Cell,
    hash::{BuildHasher, RandomState},
};

use luck_allocator::{Allocator, List};
use oxc_data_structures::types::implements;

#[test]
fn from_iter_in_holds_every_item_in_order() {
    let allocator = Allocator::new();
    let list = List::from_iter_in((1..5).map(|n| n * 10), &allocator);
    assert_eq!(*list, [10, 20, 30, 40]);
}

#[test]
fn from_array_in_holds_every_item_in_order() {
    let allocator = Allocator::new();
    let list = List::from_array_in(["a", "b"], &allocator);
    assert_eq!(*list, ["a", "b"]);
}

#[test]
fn empty_lists_allocate_nothing() {
    let allocator = Allocator::new();
    let mut scratch = vec![1, 2];
    let lists = [
        List::new(),
        List::default(),
        List::from_iter_in(Vec::new(), &allocator),
        List::from_array_in([], &allocator),
        List::split_off_in(&mut scratch, 2, &allocator),
    ];
    assert!(lists.iter().all(|list| list.is_empty()));
    assert_eq!(allocator.capacity(), 0);
    assert_eq!(scratch, [1, 2]);
}

#[test]
fn split_off_in_moves_the_tail_and_leaves_the_head() {
    let allocator = Allocator::new();
    let mut scratch = vec!["kept", "first", "second"];
    let capacity = scratch.capacity();
    let list = List::split_off_in(&mut scratch, 1, &allocator);
    assert_eq!(*list, ["first", "second"]);
    assert_eq!(scratch, ["kept"]);
    assert_eq!(scratch.capacity(), capacity);
    assert_eq!(allocator.used_bytes(), size_of::<&str>() * 2);
}

#[test]
fn split_off_in_moves_a_long_tail_in_one_copy() {
    let allocator = Allocator::new();
    let mut scratch: Vec<u64> = (0..200).collect();
    let capacity = scratch.capacity();
    let list = List::split_off_in(&mut scratch, 100, &allocator);
    assert!(list.iter().copied().eq(100..200));
    assert!(scratch.iter().copied().eq(0..100));
    assert_eq!(scratch.capacity(), capacity);
    assert_eq!(allocator.used_bytes(), size_of::<u64>() * 100);
}

#[test]
fn split_off_in_from_the_start_takes_everything() {
    let allocator = Allocator::new();
    let mut scratch = vec![1_u32, 2, 3];
    let list = List::split_off_in(&mut scratch, 0, &allocator);
    assert_eq!(*list, [1, 2, 3]);
    assert_eq!(scratch, [] as [u32; 0]);
}

#[test]
fn truncate_shortens_in_place() {
    let allocator = Allocator::new();
    let mut list = List::from_array_in([1, 2, 3], &allocator);
    let first = list.as_ptr();
    list.truncate(5);
    assert_eq!(*list, [1, 2, 3]);
    list.truncate(1);
    assert_eq!(*list, [1]);
    assert_eq!(list.as_ptr(), first);
    list.truncate(0);
    assert!(list.is_empty());
}

#[test]
fn push_grows_an_item_at_a_time_by_doubling() {
    let allocator = Allocator::new();
    let mut list = List::new();
    for n in 0..100_u64 {
        list.push(n, &allocator);
    }
    assert!(list.iter().copied().eq(0..100));
    // 4, 8, 16, 32, 64, and 128 slots, each left behind by the next.
    assert_eq!(allocator.used_bytes(), size_of::<u64>() * 252);
}

#[test]
fn push_onto_a_frozen_list_keeps_its_items() {
    let allocator = Allocator::new();
    let mut list = List::from_array_in([1, 2, 3], &allocator);
    list.push(4, &allocator);
    list.push(5, &allocator);
    assert_eq!(*list, [1, 2, 3, 4, 5]);
}

#[test]
fn insert_shifts_the_items_after_it() {
    let allocator = Allocator::new();
    let mut list = List::from_array_in([2, 4], &allocator);
    list.insert(0, 1, &allocator);
    list.insert(2, 3, &allocator);
    list.insert(4, 5, &allocator);
    assert_eq!(*list, [1, 2, 3, 4, 5]);
}

#[test]
fn split_off_divides_the_list_in_place() {
    let allocator = Allocator::new();
    let mut head = List::from_array_in([1, 2, 3, 4], &allocator);
    head.push(5, &allocator);
    let used = allocator.used_bytes();
    let mut tail = head.split_off(2);
    assert_eq!((&*head, &*tail), ([1, 2].as_slice(), [3, 4, 5].as_slice()));
    assert_eq!(allocator.used_bytes(), used);
    // The tail grows into the spare capacity it took, and the head moves out of its way.
    tail.push(6, &allocator);
    assert_eq!(allocator.used_bytes(), used);
    head.push(9, &allocator);
    assert_eq!((&*head, &*tail), ([1, 2, 9].as_slice(), [3, 4, 5, 6].as_slice()));
    assert!(head.split_off(3).is_empty());
}

#[test]
fn into_iter_moves_every_item_out_in_order() {
    let allocator = Allocator::new();
    let list = List::from_array_in(["a", "b", "c"], &allocator);
    let mut items = list.into_iter();
    assert_eq!(items.len(), 3);
    assert_eq!(items.next(), Some("a"));
    assert_eq!(items.len(), 2);
    assert_eq!(items.collect::<Vec<_>>(), ["b", "c"]);
}

#[test]
fn into_iter_refills_a_list_of_known_length() {
    let allocator = Allocator::new();
    let list = List::from_array_in([1, 2, 3], &allocator);
    let doubled = List::from_iter_in(list.into_iter().map(|n| n * 2), &allocator);
    assert_eq!(*doubled, [2, 4, 6]);
}

#[test]
fn zero_sized_items_never_allocate() {
    let allocator = Allocator::new();
    let mut list = List::new();
    for _ in 0..1000 {
        list.push((), &allocator);
    }
    list.insert(0, (), &allocator);
    assert_eq!(list.len(), 1001);
    assert_eq!(list.into_iter().count(), 1001);
    assert_eq!(allocator.used_bytes(), 0);
}

#[test]
fn items_mutate_through_the_list() {
    let allocator = Allocator::new();
    let mut list = List::from_array_in([1, 2, 3], &allocator);
    list[0] = 10;
    for item in &mut list {
        *item += 1;
    }
    list.swap(1, 2);
    assert_eq!((&list).into_iter().copied().collect::<Vec<_>>(), [11, 4, 3]);
}

#[test]
fn compares_hashes_and_prints_by_content() {
    let first = Allocator::new();
    let second = Allocator::new();
    let left = List::from_array_in([1, 2], &first);
    let right = List::from_iter_in(vec![1, 2], &second);
    assert_eq!(left, right);
    assert_ne!(left, List::from_array_in([1, 3], &second));

    let hasher = RandomState::new();
    assert_eq!(hasher.hash_one(&left), hasher.hash_one(&right));
    assert_eq!(hasher.hash_one(&left), hasher.hash_one([1, 2].as_slice()));

    assert_eq!(format!("{left:?}"), "[1, 2]");
}

#[test]
fn auto_traits_follow_the_item_type() {
    assert!(implements!(List<u32>: Send));
    assert!(implements!(List<u32>: Sync));
    assert!(implements!(List<Cell<u32>>: Send));
    assert!(implements!(List<Cell<u32>>: !Sync));
}

#[test]
fn lifetime_variance() {
    fn _shorten<'long: 'short, 'short>(list: List<'long, &'long str>) -> List<'short, &'short str> {
        list
    }
}
