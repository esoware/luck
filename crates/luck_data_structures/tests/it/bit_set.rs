use luck_data_structures::BitSet;

#[test]
fn holds_each_bit_it_sets_and_no_other() {
    let mut bits = BitSet::new(130);
    for bit in [0, 63, 64, 129] {
        assert!(bits.insert(bit), "{bit} was clear");
    }
    let set: Vec<usize> = (0..140).filter(|&bit| bits.contains(bit)).collect();
    assert_eq!(set, [0, 63, 64, 129]);
    assert!(!bits.insert(500));
    assert!(!bits.contains(500), "a bit past the end stays clear");
}

#[test]
fn inserting_a_set_bit_says_it_was_set_and_changes_nothing() {
    let mut bits = BitSet::new(70);
    assert!(bits.insert(65));
    assert!(!bits.insert(65));
    let set: Vec<usize> = (0..70).filter(|&bit| bits.contains(bit)).collect();
    assert_eq!(set, [65]);
}
