use luck_data_structures::BitSet;

#[test]
fn holds_each_bit_it_sets_and_no_other() {
    let mut bits = BitSet::new(130);
    for bit in [0, 63, 64, 129] {
        bits.insert(bit);
    }
    let set: Vec<usize> = (0..140).filter(|&bit| bits.contains(bit)).collect();
    assert_eq!(set, [0, 63, 64, 129]);
    bits.insert(500);
    assert!(!bits.contains(500), "a bit past the end stays clear");
}
