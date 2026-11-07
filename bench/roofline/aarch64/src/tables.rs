//! The kernels' stand-in for cralloc's front: slabs at the real layout, a magazine per class, and
//! the tables the C-shaped routines read. Only the fast path's traffic matters here, so every
//! magazine has one size and is refilled before each repetition.
use crate::env::Env;

pub const SLAB_SHIFT: usize = 12;
pub const SLAB_COUNT: usize = 8192;
/// Class index of the constant-class kernels, and its hot word's offset in a slab.
pub const CONST_CLASS: usize = 4;
pub const CONST_WORD: usize = 32 * CONST_CLASS + 24;
/// Entries of every magazine: its mask is `8 * 64 - 16`.
pub const MAG_ENTRIES: usize = 64;
pub const MAG_MASK: u32 = (8 * MAG_ENTRIES - 16) as u32;
/// How many entries a prepared magazine holds, with room for a batch either way.
pub const MAG_FILL: usize = 34;
/// How many class indexes have a magazine.
pub const CLASSES: usize = 44;
pub const TCM_CLASS: usize = 3;
/// A region's bytes, so that a C free finds a class from an address.
pub const REGION_SHIFT: usize = 24;
const OBJ: usize = 64;

extern "C" {
    pub static mut rf_slabs: [u8; (SLAB_COUNT + 16) << SLAB_SHIFT];
    pub static mut rf_tables: [u8; 1 << 21];
    pub static mut rf_abort_count: u64;
}

pub fn slab(cpu: usize) -> *mut u8 {
    unsafe { (&raw mut rf_slabs as *mut u8).add(cpu << SLAB_SHIFT) }
}

unsafe fn obj(env: &Env, class: usize, i: usize) -> u64 {
    env.arena as u64 + ((class << REGION_SHIFT) + i * OBJ) as u64
}

/// A fresh magazine for every class on this CPU.
pub unsafe fn prepare(env: &Env) {
    let slab = slab(env.cpu);
    for class in 1..=CLASSES {
        let mag = env.mags.add(class * 8 * MAG_ENTRIES);
        for i in 0..MAG_FILL {
            (mag.add(8 * (1 + i)) as *mut u64).write(obj(env, class, i));
        }
        (slab.add(32 * class + 24) as *mut u64).write(mag.add(8 * (1 + MAG_FILL)) as u64);
    }
}

/// A list of objects at the constant class's word.
pub unsafe fn prepare_list(env: &Env) {
    let slab = slab(env.cpu);
    let n = 64;
    for i in 0..n {
        let next = if i + 1 < n { obj(env, CONST_CLASS, i + 1) } else { 0 };
        (obj(env, CONST_CLASS, i) as *mut u64).write(next);
    }
    (slab.add(CONST_WORD) as *mut u64).write(obj(env, CONST_CLASS, 0));
}

// Where the C routines' tables sit in `rf_tables`; the kernels' `centry.py` has the same numbers.
const SIZES: usize = 2048;
const CALLOC: usize = 8192 + 2048;
const MASKS: usize = 16384 + 2048;
const PUBLISHED: usize = 16384 + 1024;
const REGIONS: usize = 16384 + 4096 + 2048;
const SIZED: usize = 65536 + 2048;
const SIZES32: usize = 131072 + 2048;
const REGIONS32: usize = 163840 + 2048;

/// The class index cralloc's C rule gives a size of at most 32 KiB: every 8 bytes up to 64 (a size
/// of 16 or more rounded to 16), then four per doubling.
fn c_class(size: usize) -> usize {
    let rounded = if size >= 16 { (size + 15) & !15 } else { size.max(1) };
    if rounded <= 64 {
        return (rounded + 7) / 8;
    }
    let log = (usize::BITS - 1 - (rounded - 1).leading_zeros()) as usize;
    8 + (log - 6) * 4 + ((rounded - 1) >> (log - 2) & 3) + 1
}

fn word_position(class: usize) -> usize {
    4 * class + 3
}

/// The C routines' tables, once; the published words and region table name `arena`'s regions.
pub unsafe fn prepare_c(env: &Env) {
    let t = &raw mut rf_tables as *mut u8;
    let packed = |class: usize| ((MAG_MASK as u32) << 16) | (8 * word_position(class)) as u32;
    for i in 0..=4096 {
        let class = c_class(8 * i).min(CLASSES);
        t.add(SIZES + i).write(word_position(class) as u8);
        (t.add(SIZES32 + 4 * i) as *mut u32).write(packed(class));
    }
    for size in 0..=1024 {
        t.add(CALLOC + size).write(word_position(c_class(size).min(CLASSES)) as u8);
    }
    for size in 0..65536usize {
        let class = if size <= 32768 { c_class(size).min(CLASSES) } else { CLASSES };
        t.add(SIZED + size).write(word_position(class) as u8);
    }
    let slab0 = &raw mut rf_slabs as *mut u8;
    for class in 1..=CLASSES {
        let pos = word_position(class);
        (t.add(MASKS + 8 * pos) as *mut u64).write(MAG_MASK as u64);
        (slab0.add(2048 + 8 * pos) as *mut u64).write(MAG_MASK as u64);
        t.add(REGIONS + class).write(pos as u8);
        (t.add(REGIONS32 + 4 * class) as *mut u32).write(packed(class));
    }
    (t.add(PUBLISHED) as *mut u64).write(REGION_SHIFT as u64);
    (t.add(PUBLISHED + 8) as *mut u64).write(env.arena as u64 >> REGION_SHIFT);
}

/// Sizes in a cycle of 64, from 1 to 512, as stream 8: what a loop of mixed sizes runs.
pub unsafe fn prepare_sizes(arr: *mut u8) {
    let base = arr.add(9 << 18) as *mut u16;
    for i in 0..65536usize {
        base.add(i).write((1 + 8 * (i % 64) + i % 8) as u16);
    }
}

/// The class of each operation for the mixed-class kernels: stream `s`, of 65536 16-bit word
/// offsets, sits `(s + 1) << 18` bytes into `arr`.
pub unsafe fn prepare_streams(arr: *mut u8) {
    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let word = |class: usize| (32 * class + 24) as u16;
    for stream in 0..8usize {
        let base = arr.add((stream + 1) << 18) as *mut u16;
        let mut last = 1usize;
        for i in 0..65536usize {
            let r = next() >> 20;
            let class = match stream {
                0 => CONST_CLASS,
                1 => 1 + i % 2,
                2 => 1 + (r % 2) as usize,
                3 => 1 + (r % 4) as usize,
                4 => 1 + (r % 8) as usize,
                5 => 1 + (r % 16) as usize,
                6 => {
                    if r % 8 == 0 {
                        last = 1 + ((r >> 8) % 8) as usize;
                    }
                    last
                }
                _ => 1 + i % 8,
            };
            base.add(i).write(word(class));
        }
    }
}

/// tcmalloc's slab: a 32-bit header per class holding `current` and `end`, the slots in units of
/// 8 bytes from the slab's start, and a sentinel below the first slot.
pub unsafe fn prepare_tcm(env: &Env) {
    let slab = env.state;
    slab.write_bytes(0, 1 << 16);
    let begin = 64usize;
    let n = 40usize;
    (slab.add(8 * (begin - 1)) as *mut u64).write(1);
    for i in 0..n {
        (slab.add(8 * (begin + i)) as *mut u64).write(obj(env, CONST_CLASS, i));
    }
    let hdr = ((begin + n) as u32) | (100u32 << 16);
    (slab.add(4 * TCM_CLASS) as *mut u32).write(hdr);
}
