//! The atomic types the front end and the depot share state through.
//!
//! In the allocator these are `core`'s atomics. In this crate's own tests each operation first
//! offers the interleaving checker (`front::check`) a point at which to switch threads, so that
//! the checker can run the lock-free protocols under every schedule it explores. Outside a
//! checker run the offer does nothing. The wrappers are transparent, so a layout that holds
//! one holds the other.
//!
//! Only the operations the protocols use are wrapped; a new use adds one line here.

#[cfg(not(test))]
pub(crate) use core::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize};

/// The plain `core` atomic inside a field, for code whose interruption points are explicit:
/// the twins, which are interrupted only by `Machine::step`, so that nothing can come between
/// a step's check and its access.
#[cfg(not(test))]
#[inline(always)]
pub(crate) fn quiet<T>(atomic: &AtomicPtr<T>) -> &core::sync::atomic::AtomicPtr<T> {
    atomic
}

#[cfg(test)]
pub(crate) fn quiet<T>(atomic: &AtomicPtr<T>) -> &core::sync::atomic::AtomicPtr<T> {
    &atomic.0
}

#[cfg(test)]
pub(crate) use self::checked::{AtomicPtr, AtomicU64, AtomicUsize};

#[cfg(test)]
mod checked {
    use super::super::check::switch_point;
    use core::sync::atomic::{self, Ordering};

    /// `core::sync::atomic::AtomicPtr`, with a switch point before each operation.
    #[repr(transparent)]
    pub(crate) struct AtomicPtr<T>(pub(super) atomic::AtomicPtr<T>);

    impl<T> AtomicPtr<T> {
        pub(crate) const fn new(value: *mut T) -> AtomicPtr<T> {
            AtomicPtr(atomic::AtomicPtr::new(value))
        }

        /// As `core::sync::atomic::AtomicPtr::from_ptr`.
        ///
        /// # Safety
        ///
        /// As for `core::sync::atomic::AtomicPtr::from_ptr`.
        pub(crate) unsafe fn from_ptr<'a>(ptr: *mut *mut T) -> &'a AtomicPtr<T> {
            // SAFETY: `AtomicPtr<T>` is a transparent wrapper of the core type, so a reference
            // to one is a reference to the other; the core function's conditions are the
            // caller's.
            unsafe { &*(atomic::AtomicPtr::from_ptr(ptr) as *const atomic::AtomicPtr<T>).cast() }
        }

        pub(crate) fn load(&self, order: Ordering) -> *mut T {
            switch_point();
            self.0.load(order)
        }

        pub(crate) fn store(&self, value: *mut T, order: Ordering) {
            switch_point();
            self.0.store(value, order)
        }

        pub(crate) fn swap(&self, value: *mut T, order: Ordering) -> *mut T {
            switch_point();
            self.0.swap(value, order)
        }

        pub(crate) fn compare_exchange(
            &self,
            current: *mut T,
            new: *mut T,
            success: Ordering,
            failure: Ordering,
        ) -> Result<*mut T, *mut T> {
            switch_point();
            self.0.compare_exchange(current, new, success, failure)
        }
    }

    macro_rules! integer {
        ($name:ident, $int:ty) => {
            /// The `core::sync::atomic` type of the same name, with a switch point before each
            /// operation.
            #[repr(transparent)]
            pub(crate) struct $name(atomic::$name);

            // The two integer types share one list of operations; neither uses all of them.
            #[allow(dead_code)]
            impl $name {
                pub(crate) const fn new(value: $int) -> $name {
                    $name(atomic::$name::new(value))
                }

                pub(crate) fn load(&self, order: Ordering) -> $int {
                    switch_point();
                    self.0.load(order)
                }

                pub(crate) fn store(&self, value: $int, order: Ordering) {
                    switch_point();
                    self.0.store(value, order)
                }

                pub(crate) fn swap(&self, value: $int, order: Ordering) -> $int {
                    switch_point();
                    self.0.swap(value, order)
                }

                pub(crate) fn compare_exchange(
                    &self,
                    current: $int,
                    new: $int,
                    success: Ordering,
                    failure: Ordering,
                ) -> Result<$int, $int> {
                    switch_point();
                    self.0.compare_exchange(current, new, success, failure)
                }

                pub(crate) fn fetch_add(&self, value: $int, order: Ordering) -> $int {
                    switch_point();
                    self.0.fetch_add(value, order)
                }

                pub(crate) fn fetch_max(&self, value: $int, order: Ordering) -> $int {
                    switch_point();
                    self.0.fetch_max(value, order)
                }

                pub(crate) fn fetch_or(&self, value: $int, order: Ordering) -> $int {
                    switch_point();
                    self.0.fetch_or(value, order)
                }
            }
        };
    }

    integer!(AtomicU64, u64);
    integer!(AtomicUsize, usize);
}
