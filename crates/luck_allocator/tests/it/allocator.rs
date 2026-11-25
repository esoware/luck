use std::{alloc::Layout, cmp::max, fmt::Debug, panic::AssertUnwindSafe, ptr};

use luck_allocator::Allocator;
use quickcheck::{Arbitrary, Gen};

use crate::common::{check, panic_message};

/// The least alignment the arena gives an allocation, so a cursor that starts at a multiple of it
/// stays at one.
const MIN_ALIGN: usize = 8;

/// One, two, or four `T`s or `U`s, allocated together.
#[derive(Clone, Debug)]
enum Elements<T, U> {
    OneT(T),
    TwoT(T, T),
    FourT(T, T, T, T),
    OneU(U),
    TwoU(U, U),
    FourU(U, U, U, U),
}

impl<T, U> Arbitrary for Elements<T, U>
where
    T: Arbitrary + Clone,
    U: Arbitrary + Clone,
{
    fn arbitrary(g: &mut Gen) -> Self {
        match u8::arbitrary(g) % 6 {
            0 => Self::OneT(T::arbitrary(g)),
            1 => Self::TwoT(T::arbitrary(g), T::arbitrary(g)),
            2 => Self::FourT(T::arbitrary(g), T::arbitrary(g), T::arbitrary(g), T::arbitrary(g)),
            3 => Self::OneU(U::arbitrary(g)),
            4 => Self::TwoU(U::arbitrary(g), U::arbitrary(g)),
            _ => Self::FourU(U::arbitrary(g), U::arbitrary(g), U::arbitrary(g), U::arbitrary(g)),
        }
    }
}

/// The addresses `value` takes, from its first byte to one past its last.
fn address_range<T>(value: &T) -> (usize, usize) {
    let start = ptr::from_ref(value).addr();
    (start, start + size_of::<T>())
}

fn overlap((a_start, a_end): (usize, usize), (b_start, b_end): (usize, usize)) -> bool {
    assert!(a_start < a_end && b_start < b_end, "both ranges hold something");
    a_start < b_end && b_start < a_end
}

#[test]
fn big_values_read_back() {
    fn property(values: Vec<u64>) {
        let allocator = Allocator::new();
        let allocated: Vec<(u64, &[u64; 32])> =
            values.into_iter().map(|value| (value, &*allocator.alloc([value; 32]))).collect();
        for (value, big) in allocated {
            assert_eq!(*big, [value; 32]);
        }
    }
    check::<fn(Vec<u64>)>(property);
}

#[test]
fn big_allocations_never_overlap() {
    fn property(values: Vec<u64>) {
        let allocator = Allocator::new();
        let mut ranges = Vec::new();
        for value in values {
            let range = address_range(allocator.alloc([value; 32]));
            assert!(ranges.iter().all(|&other| !overlap(other, range)));
            ranges.push(range);
        }
    }
    check::<fn(Vec<u64>)>(property);
}

#[test]
fn allocations_of_mixed_sizes_and_alignments_never_overlap() {
    fn property(things: Vec<Elements<u8, u64>>) {
        let allocator = Allocator::new();
        let mut ranges = Vec::new();
        for thing in things {
            let range = match thing {
                Elements::OneT(a) => address_range(allocator.alloc(a)),
                Elements::TwoT(a, b) => address_range(allocator.alloc([a, b])),
                Elements::FourT(a, b, c, x) => address_range(allocator.alloc([a, b, c, x])),
                Elements::OneU(a) => address_range(allocator.alloc(a)),
                Elements::TwoU(a, b) => address_range(allocator.alloc([a, b])),
                Elements::FourU(a, b, c, x) => address_range(allocator.alloc([a, b, c, x])),
            };
            assert!(ranges.iter().all(|&other| !overlap(range, other)));
            ranges.push(range);
        }
    }
    check::<fn(Vec<Elements<u8, u64>>)>(property);
}

#[test]
fn slices_hold_their_items_and_never_overlap() {
    fn property(slices: Vec<(u8, usize)>) {
        let allocator = Allocator::new();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (value, length) in slices {
            let count = length % 100;
            let slice = allocator.alloc_slice_copy(&vec![value; count]);
            assert_eq!(slice.len(), count);
            assert!(slice.iter().all(|&item| item == value));
            let range = (slice.as_ptr().addr(), slice.as_ptr().addr() + slice.len());
            assert!(ranges.iter().all(|&(start, end)| range.1 <= start || end <= range.0));
            ranges.push(range);
        }
    }
    check::<fn(Vec<(u8, usize)>)>(property);
}

#[test]
fn strs_read_back() {
    fn property(texts: Vec<String>) {
        let allocator = Allocator::new();
        let allocated: Vec<&str> = texts.iter().map(|text| allocator.alloc_str(text)).collect();
        for (text, copy) in texts.into_iter().zip(allocated) {
            assert_eq!(text, copy);
        }
    }
    check::<fn(Vec<String>)>(property);
}

/// Allocating nothing but empty slices makes no chunk.
#[test]
fn capacity_covers_every_byte_allocated() {
    fn property(lengths: Vec<usize>) {
        let allocator = Allocator::new();
        let mut total = 0;
        for length in lengths {
            let count = length % 512;
            allocator.alloc_slice_copy(&vec![0_i32; count]);
            total += count * size_of::<i32>();
            if total == 0 {
                assert_eq!(allocator.capacity(), 0);
            } else {
                assert!(allocator.capacity() >= total, "{} < {total}", allocator.capacity());
            }
        }
    }
    check::<fn(Vec<usize>)>(property);
}

/// Every allocation starts at a multiple of 8, so the bytes used are the bytes allocated, each run
/// of them rounded up to a multiple of 8.
#[test]
fn used_bytes_count_every_byte_allocated() {
    fn property(lengths: Vec<usize>) {
        let mut allocator = Allocator::new();
        let mut total = 0;
        for length in lengths {
            let count = length % 512;
            allocator.alloc_slice_copy(&vec![0_u8; count]);
            total += count.next_multiple_of(MIN_ALIGN);
            assert_eq!(allocator.used_bytes(), total);
            assert!(allocator.used_bytes() <= allocator.capacity());
        }
        allocator.reset();
        assert_eq!(allocator.used_bytes(), 0);
    }
    check::<fn(Vec<usize>)>(property);
}

/// Allocations that share an alignment and whose sizes are multiples of it and of 8 fill each
/// chunk with no padding, so the bytes used are exactly their sizes.
#[test]
fn allocations_of_one_alignment_take_no_padding() {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "quickcheck hands a property its cases by value"
    )]
    fn property(sizes: Vec<usize>) {
        for alignment in [1, 2, 4, 8, 16] {
            let allocator = Allocator::with_capacity(513);
            let mut total = 0;
            for case in &sizes {
                let size = (case % 10) * max(alignment, MIN_ALIGN);
                let ptr = allocator.alloc_layout(Layout::from_size_align(size, alignment).unwrap());
                assert!(ptr.as_ptr().addr().is_multiple_of(alignment));
                total += size;
            }
            assert_eq!(allocator.used_bytes(), total, "alignment {alignment}");
        }
    }
    check::<fn(Vec<usize>)>(property);
}

/// Every allocation starts at a multiple of 8, so those of no size and an alignment of at most 8
/// take no room at all, even after a byte.
#[test]
fn zero_sized_allocations_take_only_their_alignment_padding() {
    struct ZeroSized;

    let allocator = Allocator::new();
    let byte = allocator.alloc_layout(Layout::new::<u8>()).as_ptr().addr();
    allocator.alloc_slice_copy::<u64>(&[]);
    allocator.alloc_layout(Layout::array::<String>(0).unwrap());
    allocator.alloc_layout(Layout::array::<u64>(0).unwrap());
    let zero_sized = ptr::from_mut(allocator.alloc(ZeroSized)).addr();
    assert_eq!(byte, zero_sized);
    let next_byte = allocator.alloc_layout(Layout::new::<u8>()).as_ptr().addr();
    assert_eq!(zero_sized, next_byte + MIN_ALIGN);
}

/// No `Layout` can always wrap the cursor below address 0 on a 32-bit target, where the heap may
/// sit in the top half of the address space, and the fast path's unit tests cover it there.
#[cfg(target_pointer_width = "64")]
#[test]
fn oom_instead_of_bump_pointer_underflow() {
    let allocator = Allocator::new();
    let address = ptr::from_mut(allocator.alloc(0_u8)).addr();
    assert!(
        address < isize::MAX.unsigned_abs(),
        "the arena is in the top half of the address space, at {address:#x}, where this tests nothing"
    );
    // Big enough to take the cursor below address 0.
    let largest = Layout::from_size_align(isize::MAX.unsigned_abs(), 1).unwrap();
    let message = panic_message(AssertUnwindSafe(|| {
        allocator.alloc_layout(largest);
    }));
    assert_eq!(message, "out of memory");
}

#[test]
fn allocations_past_the_default_chunk_size_get_chunks_that_fit() {
    let allocator = Allocator::new();
    allocator.alloc_layout(Layout::from_size_align(1, 1).unwrap());
    allocator.alloc_layout(Layout::from_size_align(100_001, 1).unwrap());
    allocator.alloc_layout(Layout::from_size_align(100_003, 1).unwrap());
}

/// 64 is about the strongest alignment in practice, as AVX-512 types or cache line padding take.
#[test]
fn alloc_with_strong_alignment() {
    let allocator = Allocator::new();
    allocator.alloc_layout(Layout::from_size_align(4096, 64).unwrap());
}

#[test]
fn small_size_and_large_align() {
    let allocator = Allocator::new();
    allocator.alloc_layout(Layout::from_size_align(1, 0x1000).unwrap());
}

#[test]
fn alloc_layout_aligns_every_allocation() {
    for alignment in [2, 4, 8, 16, 32, 64] {
        let allocator = Allocator::with_capacity(513);
        let layout = Layout::from_size_align(alignment, alignment).unwrap();
        for _ in 0..1024 {
            let address = allocator.alloc_layout(layout).as_ptr().addr();
            assert!(address.is_multiple_of(alignment), "{address:#x} at alignment {alignment}");
        }
    }
}

#[test]
fn alloc_slice_copy_copies_every_item() {
    let allocator = Allocator::new();
    let items: &[u16] = &[0xFEED, 0xFACE, 0xA7, 0xCAFE];
    assert_eq!(allocator.alloc_slice_copy(items), items);
}

/// A value narrower than 8 bytes takes 8, and no wider one takes any padding.
#[test]
fn with_capacity_holds_values_of_every_size() {
    fn allocate_in_order<T: Copy + Debug + Eq>(values: &(impl Iterator<Item = T> + Clone)) {
        for capacity in [0, 1, 8, 11, 0x1000, 0x12345] {
            let allocator = Allocator::with_capacity(capacity);
            assert!(allocator.capacity() >= capacity);
            let allocated: Vec<&T> = values.clone().map(|value| &*allocator.alloc(value)).collect();
            assert!(values.clone().eq(allocated.iter().map(|&&value| value)));
            let size = max(size_of::<T>(), MIN_ALIGN);
            assert_eq!(allocator.used_bytes(), allocated.len() * size, "{capacity}");
        }
    }
    allocate_in_order(&(0_u8..255));
    allocate_in_order(&(0_u16..10_000));
    allocate_in_order(&(0_u32..10_000));
    allocate_in_order(&(0_u64..10_000));
    allocate_in_order(&(0_u128..10_000));
}

#[test]
fn with_capacity_past_isize_max_runs_out_of_memory() {
    let message = panic_message(|| {
        Allocator::with_capacity(isize::MAX.unsigned_abs() + 1);
    });
    assert_eq!(message, "out of memory");
}

/// Each value lands 8 bytes below the one before, at a multiple of 8, however narrow it is.
#[test]
fn used_bytes_count_alignment_padding() {
    let mut allocator = Allocator::with_capacity(64 * 1024);
    allocator.alloc(1_u8);
    allocator.alloc(2_u8);
    allocator.alloc(3_u32);
    assert_eq!(allocator.used_bytes(), 3 * MIN_ALIGN);
    allocator.reset();
    assert_eq!(allocator.used_bytes(), 0);
}

#[test]
fn concat_strs_array_of_one() {
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array(["hello"]), "hello");
}

#[test]
fn concat_strs_array_of_two() {
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array(["hello", "world!"]), "helloworld!");
}

#[test]
fn concat_strs_array_of_borrowed_and_owned() {
    let world = String::from("world");
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array(["hello", &world, "!"]), "helloworld!");
}

#[test]
fn concat_strs_array_of_none() {
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array([]), "");
}

#[test]
fn concat_strs_array_of_empty_strs() {
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array(["", "", ""]), "");
}

#[test]
fn concat_strs_array_with_some_empty_strs() {
    let allocator = Allocator::new();
    assert_eq!(allocator.alloc_concat_strs_array(["", "hello", ""]), "hello");
}
