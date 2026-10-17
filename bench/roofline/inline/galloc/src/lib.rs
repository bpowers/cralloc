//! Nothing but the `#[global_allocator]` declaration. The `__rust_alloc` family is generated
//! in whichever crate holds this declaration; keeping that crate free of `#[used]` statics and
//! module-level assembly keeps those functions importable under thin LTO.
#[global_allocator]
static A: fastalloc::Fast = fastalloc::Fast;
