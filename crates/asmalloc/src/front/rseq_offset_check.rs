//! Whether the rseq offset this test binary's routines load shares a page offset with a hot word.
//!
//! The routines load the thread's rseq offset on every call, from storage the linker or the C
//! library placed: the executable's copy of glibc's `__rseq_offset` (the `executable` form), its
//! GOT slot and then glibc's variable (the default form), or, on musl, the crate's own word. If
//! any of those overlaps a hot word in the low 12 bits of its address, a call can wait for the
//! previous call's store to that hot word, and a loop of them runs one at a time
//! (`docs/research/zen5-roofline.md`, section 16). The layout keeps hot words at bytes 24 to 31
//! of each 32-byte chunk, which clears any 16-aligned word; this test catches a link where that
//! assumption failed. `scripts/rseq-offset-check` does the same for other binaries.

use super::{SLAB_BYTES, clear_of_hot};
use std::string::String;
use std::vec::Vec;

/// Where this binary's relocations put something for `__rseq_offset`: (relocation type,
/// address in the file's image). A GOT slot (`R_X86_64_GLOB_DAT`) or a copy (`R_X86_64_COPY`).
fn rseq_offset_relocations(image: &[u8]) -> Vec<(u32, u64)> {
    let u16_at = |at: usize| u16::from_le_bytes(image[at..at + 2].try_into().unwrap()) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(image[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(image[at..at + 8].try_into().unwrap());
    assert_eq!(&image[..5], b"\x7fELF\x02", "an ELF64 file");
    let (shoff, shentsize, shnum) = (u64_at(0x28) as usize, u16_at(0x3a), u16_at(0x3c));
    let section = |i: usize| shoff + i * shentsize;
    let offset = |s: usize| u64_at(s + 0x18) as usize;
    let size = |s: usize| u64_at(s + 0x20) as usize;
    let link = |s: usize| u32_at(s + 0x28) as usize;
    let mut found = Vec::new();
    for i in 0..shnum {
        let rela = section(i);
        // SHT_RELA
        if u32_at(rela + 4) != 4 || link(rela) == 0 {
            continue;
        }
        let symtab = section(link(rela));
        let strtab = section(link(symtab));
        for r in (offset(rela)..offset(rela) + size(rela)).step_by(24) {
            let info = u64_at(r + 8);
            let sym = offset(symtab) + 24 * (info >> 32) as usize;
            let name = offset(strtab) + u32_at(sym) as usize;
            let end = name + image[name..].iter().position(|&b| b == 0).unwrap();
            if &image[name..end] == b"__rseq_offset" {
                found.push((info as u32, u64_at(r)));
            }
        }
    }
    found
}

#[test]
fn the_rseq_offset_the_routines_load_is_off_every_hot_words_page_offset() {
    let image = std::fs::read("/proc/self/exe").expect("read the test binary");
    let relocations = rseq_offset_relocations(&image);
    // With a copy (the `executable` form) the inlined routines read the copy, and only the
    // out-of-line ones read through the GOT slot.
    let copied = relocations.iter().any(|&(kind, _)| kind == 5);
    let mut sites: Vec<(String, usize)> = relocations
        .into_iter()
        .filter(|&(kind, _)| !(copied && kind == 6))
        .map(|(kind, at)| {
            let what = match kind {
                5 => "the copy of `__rseq_offset` (R_X86_64_COPY)",
                6 => "the GOT slot of `__rseq_offset` (R_X86_64_GLOB_DAT)",
                _ => "a relocation of `__rseq_offset`",
            };
            (what.into(), at as usize)
        })
        .collect();
    // The load bias is a whole number of pages, so file addresses keep their page offsets; the
    // variable itself is the copy, glibc's own, or on musl the crate's word.
    let variable = (&raw const super::__rseq_offset).addr();
    sites.push((
        "the variable the routines read, at run time".into(),
        variable,
    ));
    for (what, at) in sites {
        assert!(
            clear_of_hot(at % SLAB_BYTES, 8),
            "{what} is at page offset {:#x}, which overlaps a slab hot word's page offset. Every \
             fast path loads it right after the previous call's store to a hot word, and can then \
             wait for that store: allocation loops of that class may run several times slower, \
             depending on the predictor (docs/research/zen5-roofline.md, section 16). The layout \
             assumes the linker and the C library place it 16-aligned.",
            at % SLAB_BYTES
        );
    }
}

/// `scripts/rseq-offset-check` lists the hot words' bytes itself; it must agree with the slab.
#[test]
fn the_check_script_knows_where_the_hot_words_are() {
    use crate::classes::SMALL_COUNT;
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/rseq-offset-check"
    );
    let text = std::fs::read_to_string(script).expect("read scripts/rseq-offset-check");
    let line = std::format!(
        "HOT_WORDS = [32 * c + 24 for c in range(1, {})]",
        SMALL_COUNT + 1
    );
    assert!(text.contains(&line), "the script should say `{line}`");
    for class in crate::classes::SmallClass::all() {
        let at = super::Field::hot(class).offset();
        assert_eq!(at, 32 * class.class().index() as usize + 24);
    }
}
