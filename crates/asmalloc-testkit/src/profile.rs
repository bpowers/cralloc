//! The shapes of operation streams.

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// Relative weights of the operations a stream draws. A zero weight disables one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weights {
    /// Plain allocation.
    pub alloc: u32,
    /// Zeroed allocation.
    pub zeroed: u32,
    /// Free.
    pub free: u32,
    /// Resize (`realloc`, `grow`, `shrink`).
    pub resize: u32,
    /// Check a block and rewrite its contents.
    pub touch: u32,
    /// Continue the stream on a fresh thread and let the current one exit, so later frees of
    /// blocks it allocated happen on another thread after it is gone.
    pub migrate: u32,
}

/// How request sizes are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sizes {
    /// One of these sizes, uniformly: the fixed layouts of `Box<T>` and collection nodes.
    Fixed(&'static [usize]),
    /// Weights of four bands: 1 to 10 000 bytes with every doubling equally likely, then 10 KiB
    /// to 100 KiB, 100 KiB to 4 MiB, and 4 MiB to 64 MiB, each uniform.
    Bands([u32; 4]),
}

/// Number of alignment classes in [`Profile::aligns`].
pub const ALIGN_CLASSES: usize = 8;

/// How a resize picks its new size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Growth {
    /// Half the time a size near the old one (in-place growth and shrinking), otherwise a fresh
    /// draw (moves between size classes).
    Near,
    /// Mostly growth by a factor of 1.25 to 2, as `Vec` and `String` grow; sometimes a shrink.
    Grow,
}

/// Which live block a free picks, and the order a sweep frees in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// A uniformly random live block.
    Random,
    /// The newest block (stack-like).
    Lifo,
    /// The oldest block (queue-like).
    Fifo,
}

/// The shape of an operation stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Name used in failure reports.
    pub name: &'static str,
    /// Operation weights.
    pub weights: Weights,
    /// Size distribution.
    pub sizes: Sizes,
    /// Weights of the alignment classes: 1 to 8 (uniform over the four powers of two), 16, 32,
    /// 64, 128 to 2048, 4096, 8192 to 65536, and 2 MiB.
    pub aligns: [u32; ALIGN_CLASSES],
    /// How resizes pick sizes.
    pub growth: Growth,
    /// How frees pick blocks.
    pub order: Order,
    /// Drawn sizes are clamped to this (for fuzzing and Miri).
    pub max_size: usize,
    /// Drawn alignments are clamped to this.
    pub max_align: usize,
    /// Bytes requested and live never exceed this, split evenly between workers. An allocation
    /// that would exceed it becomes a free, so an allocator with this much memory, plus its own
    /// overhead, never has a reason to refuse.
    pub max_live_bytes: usize,
    /// Non-zero for churn: allocate until this many blocks are live, then free until fewer are.
    pub live_target: usize,
    /// Non-zero for batches: allocate up to this many blocks, free all of them in `order`, repeat.
    pub batch: usize,
    /// Free every live block every this many operations; zero never sweeps.
    pub sweep_every: usize,
}

const RUST_ALIGNS: [u32; ALIGN_CLASSES] = [800, 80, 40, 40, 20, 10, 10, 0];

impl Profile {
    /// Small fixed sizes, as `Box<T>` and tree nodes allocate them, with steady churn.
    pub const SMALL_FIXED: Profile = Profile {
        name: "small_fixed",
        weights: Weights {
            alloc: 450,
            zeroed: 50,
            free: 450,
            resize: 20,
            touch: 30,
            migrate: 0,
        },
        sizes: Sizes::Fixed(&[8, 16, 24, 32, 40, 48, 64, 80, 96, 128, 192, 256, 512]),
        aligns: [950, 50, 0, 0, 0, 0, 0, 0],
        growth: Growth::Near,
        order: Order::Random,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 64 * MIB,
        live_target: 0,
        batch: 0,
        sweep_every: 0,
    };

    /// Every size band with a realistic skew toward small blocks, plenty of resizing, and an
    /// occasional thread migration.
    pub const MIXED: Profile = Profile {
        name: "mixed",
        weights: Weights {
            alloc: 350,
            zeroed: 100,
            free: 330,
            resize: 150,
            touch: 69,
            migrate: 1,
        },
        sizes: Sizes::Bands([920, 50, 25, 5]),
        aligns: RUST_ALIGNS,
        growth: Growth::Near,
        order: Order::Random,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 128 * MIB,
        live_target: 0,
        batch: 0,
        sweep_every: 5000,
    };

    /// Large blocks and buffers that keep growing, as vectors and strings do.
    pub const LARGE_GROWING: Profile = Profile {
        name: "large_growing",
        weights: Weights {
            alloc: 250,
            zeroed: 50,
            free: 250,
            resize: 400,
            touch: 50,
            migrate: 0,
        },
        sizes: Sizes::Bands([200, 300, 450, 50]),
        aligns: [900, 50, 20, 20, 10, 0, 0, 0],
        growth: Growth::Grow,
        order: Order::Random,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 128 * MIB,
        live_target: 0,
        batch: 0,
        sweep_every: 500,
    };

    /// Every alignment class, up to 2 MiB.
    pub const ALIGN_HEAVY: Profile = Profile {
        name: "align_heavy",
        weights: Weights {
            alloc: 400,
            zeroed: 100,
            free: 400,
            resize: 50,
            touch: 50,
            migrate: 0,
        },
        sizes: Sizes::Bands([900, 80, 20, 0]),
        aligns: [200, 150, 150, 150, 150, 100, 90, 10],
        growth: Growth::Near,
        order: Order::Random,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 128 * MIB,
        live_target: 0,
        batch: 0,
        sweep_every: 2000,
    };

    /// Churn among a fixed population of a thousand live blocks: every allocation is paid for
    /// by a free, the steady state of a long-running service.
    pub const CHURN: Profile = Profile {
        name: "churn",
        weights: Weights {
            alloc: 450,
            zeroed: 50,
            free: 450,
            resize: 30,
            touch: 20,
            migrate: 0,
        },
        sizes: Sizes::Bands([980, 20, 0, 0]),
        aligns: RUST_ALIGNS,
        growth: Growth::Near,
        order: Order::Random,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 64 * MIB,
        live_target: 1000,
        batch: 0,
        sweep_every: 0,
    };

    /// Allocate a batch, free it newest first, repeat: stack-shaped lifetimes.
    pub const LIFO_BATCHES: Profile = Profile {
        name: "lifo_batches",
        weights: Weights {
            alloc: 450,
            zeroed: 50,
            free: 0,
            resize: 100,
            touch: 50,
            migrate: 0,
        },
        sizes: Sizes::Bands([950, 50, 0, 0]),
        aligns: RUST_ALIGNS,
        growth: Growth::Near,
        order: Order::Lifo,
        max_size: usize::MAX,
        max_align: usize::MAX,
        max_live_bytes: 64 * MIB,
        live_target: 0,
        batch: 256,
        sweep_every: 0,
    };

    /// Allocate a batch, free it oldest first, repeat: queue-shaped lifetimes.
    pub const FIFO_BATCHES: Profile = Profile {
        name: "fifo_batches",
        order: Order::Fifo,
        ..Profile::LIFO_BATCHES
    };

    /// Every preset.
    pub const ALL: [Profile; 7] = [
        Profile::SMALL_FIXED,
        Profile::MIXED,
        Profile::LARGE_GROWING,
        Profile::ALIGN_HEAVY,
        Profile::CHURN,
        Profile::LIFO_BATCHES,
        Profile::FIFO_BATCHES,
    ];

    /// The preset with this name.
    pub fn named(name: &str) -> Option<Profile> {
        Profile::ALL.into_iter().find(|p| p.name == name)
    }

    /// This profile with sizes, alignments and live bytes capped, for fuzzing and Miri.
    pub const fn capped(self, max_size: usize, max_align: usize, max_live_bytes: usize) -> Self {
        Profile {
            max_size,
            max_align,
            max_live_bytes,
            ..self
        }
    }

    /// This profile with a different migration weight.
    pub const fn with_migrate(mut self, weight: u32) -> Self {
        self.weights.migrate = weight;
        self
    }

    /// Panics if the profile cannot produce a stream.
    pub fn validate(&self) {
        let w = self.weights;
        assert!(
            w.alloc + w.zeroed > 0,
            "{}: no allocation weight",
            self.name
        );
        match self.sizes {
            Sizes::Fixed(t) => assert!(!t.is_empty(), "{}: no sizes", self.name),
            Sizes::Bands(b) => assert!(b.iter().sum::<u32>() > 0, "{}: no size", self.name),
        }
        assert!(
            self.aligns.iter().sum::<u32>() > 0,
            "{}: no alignment",
            self.name
        );
        assert!(
            self.max_size > 0 && self.max_align > 0,
            "{}: zero cap",
            self.name
        );
        assert!(self.max_live_bytes > 0, "{}: zero live-byte cap", self.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid_and_named_uniquely() {
        for p in Profile::ALL {
            p.validate();
            assert_eq!(Profile::named(p.name), Some(p));
        }
    }
}
