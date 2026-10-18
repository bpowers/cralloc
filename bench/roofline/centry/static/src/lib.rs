//! asmalloc's C interface as a static library, for linking into the C loops (`../cloop.c`) so
//! that they call the generated routines directly, as a statically linked program does.
#![no_std]

asmalloc::export_c_abi!();

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    use asmalloc::os::Os;
    asmalloc::os::linux::LinuxOs.abort("centry-static: panic")
}
