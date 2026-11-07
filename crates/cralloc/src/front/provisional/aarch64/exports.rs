//! The C symbols of the routines for aarch64: each a copy of the routine it exports.

/// Defines `malloc`, `calloc`, `free` and `free_sized` as C symbols.
#[doc(hidden)]
#[macro_export]
macro_rules! cralloc_exports_c_aarch64 {
    () => {
        // SAFETY: PROVISIONAL-02; the exported name is C-EXPORT-02.
        /// # Safety
        ///
        /// As for this C function, in `cralloc::c`.
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "C" fn calloc(n: usize, size: usize) -> usize {
            ::core::arch::naked_asm!(
                $crate::cralloc_a64_c_calloc!(),
                C_CALLOC_SIZES = sym $crate::c::table::C_CALLOC_SIZES,
                SLABS = sym $crate::front::SLABS,
                zero = sym $crate::c::slow::calloc_zero_tail,
                slow = sym $crate::c::slow::calloc_slow_args,
            )
        }

        // SAFETY: PROVISIONAL-02; the exported name is C-EXPORT-02.
        /// # Safety
        ///
        /// As for this C function, in `cralloc::c`.
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "C" fn free(ptr: usize) {
            ::core::arch::naked_asm!(
                $crate::cralloc_a64_c_free!(),
                SLABS = sym $crate::front::SLABS,
                slow = sym $crate::c::slow::free_slow,
            )
        }

        // SAFETY: PROVISIONAL-02; the exported name is C-EXPORT-02.
        /// # Safety
        ///
        /// As for this C function, in `cralloc::c`.
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "C" fn free_sized(ptr: usize, size: usize) {
            ::core::arch::naked_asm!(
                $crate::cralloc_a64_c_free_sized!(),
                C_SIZED_SIZES = sym $crate::c::table::C_SIZED_SIZES,
                SLABS = sym $crate::front::SLABS,
                slow = sym $crate::c::slow::free_sized_slow,
            )
        }

        // SAFETY: PROVISIONAL-02; the exported name is C-EXPORT-02.
        /// # Safety
        ///
        /// As for this C function, in `cralloc::c`.
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        unsafe extern "C" fn malloc(size: usize) -> usize {
            ::core::arch::naked_asm!(
                $crate::cralloc_a64_c_malloc!(),
                C_SIZES = sym $crate::c::table::C_SIZES,
                SLABS = sym $crate::front::SLABS,
                slow = sym $crate::c::slow::malloc_slow,
            )
        }
    };
}
