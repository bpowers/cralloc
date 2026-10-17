//! Division by a class size without a divide instruction.
//!
//! The C entry points and laundering turn an object's offset within its region into a slot
//! index, with a class known only at run time. A 64-bit divide costs tens of cycles; this costs
//! two shifts and a multiply.
//!
//! Every class size is `q << e` with `q` in {1, 3, 5, 7} and `e >= 3`, so
//! `offset / size == (offset >> e) / q`. Region offsets are below 2^32, so `x = offset >> e` is
//! below 2^29. With `M = ceil(2^32 / q)` and `d = q * M - 2^32`, which is less than `q`:
//!
//! ```text
//! x * M / 2^32 = x / q + x * d / (q * 2^32)
//! ```
//!
//! The fractional part of `x / q` is at most `(q - 1) / q`, and `x * d < 2^29 * 8 = 2^32`, so the
//! error term is less than `1 / q` and the floor is unchanged: `(x * M) >> 32 == x / q`. The
//! product is below `2^29 * 2^32`, so it fits in 64 bits.
//!
//! Kani checks this in three parts, because a SAT solver needs a minute or two to verify a
//! multiplier and the merge gate has a minute in all. `verify.rs`, in the gate: every class's
//! divisor is its odd factor's multiplier and a shift of at least 3; and if `quotient(x)` is
//! `x / q` then `divide(offset)` is `offset / (q << shift)`. `verify_slow.rs`, on demand:
//! `quotient` is exact for every `x` below 2^29, for each `q`.

/// `ceil(2^32 / q)` for `q = 1, 3, 5, 7`, indexed by `q >> 1`.
pub(super) const MAGIC: [u64; 4] = [1 << 32, magic(3), magic(5), magic(7)];

const fn magic(q: u64) -> u64 {
    (1u64 << 32).div_ceil(q)
}

/// Division by one class size, for offsets within a region.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Divisor {
    pub(super) shift: u32,
    pub(super) magic: u64,
}

impl Divisor {
    /// The divisor for a class size. The size must be `q << e` with `q` in {1, 3, 5, 7} and
    /// `e >= 3`, which every class size is; [`super::Class::divisor`] is the public way in.
    #[inline(always)]
    pub(super) const fn of(size: usize) -> Divisor {
        let shift = size.trailing_zeros();
        let odd = size >> shift;
        Divisor {
            shift,
            magic: MAGIC[(odd >> 1) & 3],
        }
    }

    /// `offset / size`, rounded down: the index of the slot holding byte `offset` of a region.
    #[inline(always)]
    pub const fn divide(self, offset: u32) -> u32 {
        // The shift is up to 32, so it is done in 64 bits; the result is below 2^29.
        quotient((offset as u64 >> self.shift) as u32, self.magic)
    }
}

/// `x / q` for `x` below 2^29, given `magic = MAGIC[q >> 1]`.
#[inline(always)]
pub(super) const fn quotient(x: u32, magic: u64) -> u32 {
    ((x as u64 * magic) >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::super::Class;

    #[test]
    fn division_matches_the_divide_instruction() {
        let offsets = [
            0u32,
            1,
            7,
            8,
            9,
            23,
            24,
            25,
            4095,
            4096,
            1 << 20,
            0x7fff_ffff,
        ];
        for class in Class::all() {
            let size = class.size() as u64;
            let divisor = class.divisor();
            let near_multiples = (1..=5u64).flat_map(|k| {
                let m = k * size;
                [m.saturating_sub(1), m, m + 1]
            });
            let tail = [u32::MAX as u64, u32::MAX as u64 - 1];
            for offset in offsets
                .iter()
                .map(|&o| o as u64)
                .chain(near_multiples)
                .chain(tail)
            {
                if offset > u32::MAX as u64 {
                    continue;
                }
                assert_eq!(
                    divisor.divide(offset as u32) as u64,
                    offset / size,
                    "{class:?} offset {offset}"
                );
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn division_is_exact_for_every_offset_of_a_small_region() {
        for class in Class::all().filter(|c| c.size() <= 1 << 16) {
            let divisor = class.divisor();
            for offset in 0..(1u32 << 18) {
                assert_eq!(divisor.divide(offset), offset / class.size() as u32);
            }
        }
    }
}
