//! Declares the global allocator and times an alloc+free pair at four kinds of call site.
use std::hint::black_box;

#[cfg(not(feature = "sep"))]
#[global_allocator]
static A: fastalloc::Fast = fastalloc::Fast;

#[cfg(feature = "sep")]
extern crate galloc;

#[cfg(feature = "used")]
#[used]
static KEEP: [u8; 8] = *b"keepme!!";

#[cfg(feature = "modasm")]
std::arch::global_asm!(".local app_local_sym", "app_local_sym:", "nop");

extern "C" {
    fn syscall(num: i64, ...) -> i64;
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn sched_setaffinity(pid: i32, sz: usize, mask: *const u64) -> i32;
}

struct Counter(std::fs::File);

impl Counter {
    fn cycles() -> Counter {
        use std::os::fd::FromRawFd;
        let mut attr = [0u64; 16];
        attr[0] = 128u64 << 32;
        // disabled | exclude_kernel | exclude_hv
        attr[5] = 1 | (1 << 5) | (1 << 6);
        let fd = unsafe { syscall(298, attr.as_ptr(), 0i32, -1i32, -1i32, 0u64) };
        assert!(fd >= 0, "perf_event_open failed");
        Counter(unsafe { std::fs::File::from_raw_fd(fd as i32) })
    }
    fn start(&self) {
        use std::os::fd::AsRawFd;
        unsafe {
            ioctl(self.0.as_raw_fd(), 0x2403, 0);
            ioctl(self.0.as_raw_fd(), 0x2400, 0);
        }
    }
    fn stop(&mut self) -> u64 {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        unsafe { ioctl(self.0.as_raw_fd(), 0x2401, 0) };
        let mut b = [0u8; 8];
        self.0.read_exact(&mut b).unwrap();
        u64::from_le_bytes(b)
    }
}


// rustc folds small modules into one codegen unit. These distinct, never-inlined functions make
// each module big enough to keep its own, so `site_other` really is in another unit.
macro_rules! filler {
    ($table:ident: $($name:ident $k:literal),*) => {
        $(
            #[inline(never)]
            fn $name(x: u64) -> u64 {
                let mut v = x.wrapping_mul($k).rotate_left(($k % 63) as u32) ^ $k;
                for i in 0..($k % 7 + 2) {
                    v = v.wrapping_add(i).wrapping_mul(0x9E37_79B9_7F4A_7C15 ^ $k).rotate_right(7);
                    if v & 1 == 0 { v ^= $k } else { v = v.wrapping_sub($k) }
                }
                v
            }
        )*
        pub static $table: &[fn(u64) -> u64] = &[$($name),*];
    };
}

filler!(FILL_ROOT: r0 1000003, r1 1000020, r2 1000037, r3 1000054, r4 1000071, r5 1000088, r6 1000105, r7 1000122, r8 1000139, r9 1000156, r10 1000173, r11 1000190, r12 1000207, r13 1000224, r14 1000241, r15 1000258, r16 1000275, r17 1000292, r18 1000309, r19 1000326, r20 1000343, r21 1000360, r22 1000377, r23 1000394, r24 1000411, r25 1000428, r26 1000445, r27 1000462, r28 1000479, r29 1000496, r30 1000513, r31 1000530, r32 1000547, r33 1000564, r34 1000581, r35 1000598, r36 1000615, r37 1000632, r38 1000649, r39 1000666, r40 1000683, r41 1000700, r42 1000717, r43 1000734, r44 1000751, r45 1000768, r46 1000785, r47 1000802, r48 1000819, r49 1000836, r50 1000853, r51 1000870, r52 1000887, r53 1000904, r54 1000921, r55 1000938, r56 1000955, r57 1000972, r58 1000989, r59 1001006, r60 1001023, r61 1001040, r62 1001057, r63 1001074, r64 1001091, r65 1001108, r66 1001125, r67 1001142, r68 1001159, r69 1001176, r70 1001193, r71 1001210, r72 1001227, r73 1001244, r74 1001261, r75 1001278, r76 1001295, r77 1001312, r78 1001329, r79 1001346, r80 1001363, r81 1001380, r82 1001397, r83 1001414, r84 1001431, r85 1001448, r86 1001465, r87 1001482, r88 1001499, r89 1001516, r90 1001533, r91 1001550, r92 1001567, r93 1001584, r94 1001601, r95 1001618, r96 1001635, r97 1001652, r98 1001669, r99 1001686, r100 1001703, r101 1001720, r102 1001737, r103 1001754, r104 1001771, r105 1001788, r106 1001805, r107 1001822, r108 1001839, r109 1001856, r110 1001873, r111 1001890, r112 1001907, r113 1001924, r114 1001941, r115 1001958, r116 1001975, r117 1001992, r118 1002009, r119 1002026, r120 1002043, r121 1002060, r122 1002077, r123 1002094, r124 1002111, r125 1002128, r126 1002145, r127 1002162, r128 1002179, r129 1002196, r130 1002213, r131 1002230, r132 1002247, r133 1002264, r134 1002281, r135 1002298, r136 1002315, r137 1002332, r138 1002349, r139 1002366, r140 1002383, r141 1002400, r142 1002417, r143 1002434, r144 1002451, r145 1002468, r146 1002485, r147 1002502, r148 1002519, r149 1002536, r150 1002553, r151 1002570, r152 1002587, r153 1002604, r154 1002621, r155 1002638, r156 1002655, r157 1002672, r158 1002689, r159 1002706);

/// (a) An allocation site in the crate that declares the global allocator.
#[inline(never)]
fn site_bin(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let b = black_box(Box::new([i, 1, 2, 3]));
        acc = acc.wrapping_add(b[0]);
    }
    acc
}

/// (a') The same, in another module of that crate, which lands in another codegen unit.
mod other {
    use std::hint::black_box;

    filler!(FILL_OTHER: o0 2000003, o1 2000022, o2 2000041, o3 2000060, o4 2000079, o5 2000098, o6 2000117, o7 2000136, o8 2000155, o9 2000174, o10 2000193, o11 2000212, o12 2000231, o13 2000250, o14 2000269, o15 2000288, o16 2000307, o17 2000326, o18 2000345, o19 2000364, o20 2000383, o21 2000402, o22 2000421, o23 2000440, o24 2000459, o25 2000478, o26 2000497, o27 2000516, o28 2000535, o29 2000554, o30 2000573, o31 2000592, o32 2000611, o33 2000630, o34 2000649, o35 2000668, o36 2000687, o37 2000706, o38 2000725, o39 2000744, o40 2000763, o41 2000782, o42 2000801, o43 2000820, o44 2000839, o45 2000858, o46 2000877, o47 2000896, o48 2000915, o49 2000934, o50 2000953, o51 2000972, o52 2000991, o53 2001010, o54 2001029, o55 2001048, o56 2001067, o57 2001086, o58 2001105, o59 2001124, o60 2001143, o61 2001162, o62 2001181, o63 2001200, o64 2001219, o65 2001238, o66 2001257, o67 2001276, o68 2001295, o69 2001314, o70 2001333, o71 2001352, o72 2001371, o73 2001390, o74 2001409, o75 2001428, o76 2001447, o77 2001466, o78 2001485, o79 2001504, o80 2001523, o81 2001542, o82 2001561, o83 2001580, o84 2001599, o85 2001618, o86 2001637, o87 2001656, o88 2001675, o89 2001694, o90 2001713, o91 2001732, o92 2001751, o93 2001770, o94 2001789, o95 2001808, o96 2001827, o97 2001846, o98 2001865, o99 2001884, o100 2001903, o101 2001922, o102 2001941, o103 2001960, o104 2001979, o105 2001998, o106 2002017, o107 2002036, o108 2002055, o109 2002074, o110 2002093, o111 2002112, o112 2002131, o113 2002150, o114 2002169, o115 2002188, o116 2002207, o117 2002226, o118 2002245, o119 2002264, o120 2002283, o121 2002302, o122 2002321, o123 2002340, o124 2002359, o125 2002378, o126 2002397, o127 2002416, o128 2002435, o129 2002454, o130 2002473, o131 2002492, o132 2002511, o133 2002530, o134 2002549, o135 2002568, o136 2002587, o137 2002606, o138 2002625, o139 2002644, o140 2002663, o141 2002682, o142 2002701, o143 2002720, o144 2002739, o145 2002758, o146 2002777, o147 2002796, o148 2002815, o149 2002834, o150 2002853, o151 2002872, o152 2002891, o153 2002910, o154 2002929, o155 2002948, o156 2002967, o157 2002986, o158 2003005, o159 2003024);

    #[inline(never)]
    pub fn site_other(n: u64) -> u64 {
        let mut acc = 0u64;
        for i in 0..n {
            // Different constants from `site_bin`, so the two are not folded into one function.
            let b = black_box(Box::new([i, 5, 6, 7]));
            acc = acc.wrapping_add(b[0]);
        }
        acc
    }
}

/// (d) An allocation made by code that ships precompiled in the standard library.
#[inline(never)]
fn site_std(n: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let s = black_box(format!("{i}"));
        acc = acc.wrapping_add(s.len() as u64);
    }
    acc
}

fn main() {
    let cpu: usize = std::env::var("RF_CPU").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let mask = [1u64 << cpu, 0, 0, 0];
    unsafe { sched_setaffinity(0, 32, mask.as_ptr()) };
    let mut cyc = Counter::cycles();
    let n = 20_000_000u64;
    let mut run = |what: &str, f: &dyn Fn() -> u64| {
        let mut best = u64::MAX;
        for rep in 0..4 {
            cyc.start();
            black_box(f());
            let c = cyc.stop();
            if rep > 0 {
                best = best.min(c);
            }
        }
        println!("{what} {:.2}", best as f64 / n as f64);
    };
    let fill: u64 = FILL_ROOT.iter().chain(other::FILL_OTHER.iter()).fold(1, |a, f| black_box(f(a)));
    black_box(fill);
    run("bin", &|| site_bin(n));
    run("bin2", &|| other::site_other(n));
    run("dep", &|| dep::pair(n));
    run("generic", &|| dep::pair_generic::<u64>(n));
    run("std", &|| site_std(n));
}
