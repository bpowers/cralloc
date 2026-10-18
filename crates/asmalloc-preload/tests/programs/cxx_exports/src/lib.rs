//! asmalloc's C functions and C++ operators, as the preload library will export them once the
//! throwing `new` forms exist.

asmalloc::export_c_abi!();
asmalloc::export_cxx_abi!();
