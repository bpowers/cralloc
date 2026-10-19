//! A differential test of the generated slab routines against their twins.
//!
//! Kani, Miri and the schedule explorers run the slow paths with twins in place of the
//! routines, so what they show holds for the allocator only if each twin does what its routine
//! does. The routine is proved against its specification in Lean, and the twin is checked
//! against that specification restated in Rust (`verify.rs`); this test compares the two
//! directly. It runs each routine and then its twin from the same state of one slab and two
//! magazines, and requires the same result and the same slab and magazines afterwards.
//!
//! The states are the ones the slab invariant allows. For every small class's hot word: zero,
//! a marker, and every slot of a magazine of the class's size, the empty and full edges among
//! them. For every spare head: zero, a marker, a stack of one magazine and a stack of two.
//!
//! The routines work on the static `SLABS`, on the slab of the CPU the thread is really on,
//! and the twin works on the same slabs, told the same CPU. That needs the slabs to itself:
//! in the test binary the process's heap may be using any slab at any moment. So the
//! comparison runs in a child process, as the abort tests do, and there the thread pins itself
//! to the CPU it is on, so that the routine keeps working on the slab the test arranged.
//!
//! Out of its reach: a thread with no valid CPU number, since the routines read the real one,
//! and interruptions, which the Lean proofs and the twins' own tests cover.

use super::routines::Routines;
use super::slab::{Cpu, Field, SLAB_COUNT, Slab, Spare};
use super::sync::quiet;
use super::{Real, Twin};
use crate::classes::{MagazineSize, Marker, SLOT_BYTES, SMALL_COUNT, SmallClass};
use crate::os::Pinned;
use core::fmt::Debug;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicPtr, Ordering::Relaxed};
use std::collections::BTreeMap;
use std::process::Command;
use std::string::String;
use std::vec::Vec;

/// Set in the environment of the child process that runs the comparison.
const CHILD: &str = "ASMALLOC_DIFFERENTIAL_CHILD";

/// What the child prints once every case has been compared, for the parent to find.
const DONE: &str = "every routine agreed with its twin";

/// The slots of the largest magazine.
const SLOTS: usize = MagazineSize::MAX.slots();

/// Two magazines of the largest size, each aligned to that size, so that the first `n` slots of
/// either are a magazine of `n` slots whatever the magazine size.
#[repr(C, align(8192))]
struct Magazines([AtomicPtr<u8>; 2 * SLOTS]);

const _: () = assert!(align_of::<Magazines>().is_multiple_of(MagazineSize::MAX.bytes()));

static MAGAZINES: Magazines = Magazines([const { AtomicPtr::new(ptr::null_mut()) }; 2 * SLOTS]);

/// The address of slot `slot` of magazine `magazine`, 0 or 1.
fn slot(magazine: usize, slot: usize) -> *mut u8 {
    let base = MAGAZINES.0.as_ptr().cast_mut().cast::<u8>();
    base.wrapping_add((magazine * SLOTS + slot) * SLOT_BYTES)
}

/// The fake object that slot `index` of the two magazines starts with: distinct for each slot,
/// never null and never dereferenced.
fn filler(index: usize) -> *mut u8 {
    ptr::without_provenance_mut(0x10_0000 + 16 * index)
}

fn marker(generation: u64) -> *mut u8 {
    ptr::without_provenance_mut(Marker::new(generation).word())
}

/// A state to start from: `word` in one field of the CPU's slab and zero in every other;
/// `link` in the first magazine's link word, and a filler in every other slot of both
/// magazines.
#[derive(Clone, Copy, Debug)]
struct State {
    field: Field,
    word: *mut u8,
    link: *mut u8,
}

/// The fields of a slab that a routine can be aimed at: each small class's hot word and its two
/// spare heads.
const FIELDS: usize = 3 * SMALL_COUNT;

/// The words a routine could change: those fields of the slab, then every slot of both
/// magazines.
type Snapshot = [usize; FIELDS + 2 * SLOTS];

/// The comparison: the slab it works on, and for each routine, by name, how many cases left
/// the state as it was and how many changed it.
struct Comparison {
    cpu: Cpu,
    fields: Vec<Field>,
    tally: BTreeMap<&'static str, [usize; 2]>,
}

impl Comparison {
    fn new(cpu: Cpu) -> Comparison {
        let fields: Vec<Field> = Slab::drained_fields().collect();
        assert_eq!(fields.len(), FIELDS);
        Comparison {
            cpu,
            fields,
            tally: BTreeMap::new(),
        }
    }

    fn arrange(&self, state: State) {
        let slab = Real.slabs().slab(self.cpu);
        for &field in &self.fields {
            quiet(slab.field(field)).store(ptr::null_mut(), Relaxed);
        }
        quiet(slab.field(state.field)).store(state.word, Relaxed);
        for (index, slot) in MAGAZINES.0.iter().enumerate() {
            slot.store(filler(index), Relaxed);
        }
        MAGAZINES.0[0].store(state.link, Relaxed);
    }

    fn snapshot(&self) -> Snapshot {
        let slab = Real.slabs().slab(self.cpu);
        let mut words = [0; FIELDS + 2 * SLOTS];
        let (fields, slots) = words.split_at_mut(FIELDS);
        for (word, &field) in fields.iter_mut().zip(&self.fields) {
            *word = quiet(slab.field(field)).load(Relaxed).addr();
        }
        for (word, slot) in slots.iter_mut().zip(&MAGAZINES.0) {
            *word = slot.load(Relaxed).addr();
        }
        words
    }

    /// Runs `op` with the routines and then with the twins, each from `state`, and requires
    /// the same result and the same slab and magazines afterwards.
    fn check<T: PartialEq + Debug>(
        &mut self,
        state: State,
        name: &'static str,
        op: impl Fn(&dyn Routines) -> T,
    ) {
        self.arrange(state);
        let before = self.snapshot();
        let routine = (op(&Real), self.snapshot());
        self.arrange(state);
        let twin = Twin::new(Real.slabs(), Pinned(self.cpu.get() as i32));
        let twin = (op(&twin), self.snapshot());
        assert_eq!(
            routine.0, twin.0,
            "{name} from {state:?}: the routine's result, then its twin's"
        );
        if let Some(at) = (0..before.len()).find(|&at| routine.1[at] != twin.1[at]) {
            let place = match self.fields.get(at) {
                Some(field) => std::format!("{field:?} of the slab"),
                None => {
                    let slot = at - FIELDS;
                    std::format!("slot {} of magazine {}", slot % SLOTS, slot / SLOTS)
                }
            };
            panic!(
                "{name} from {state:?}: {place} holds {:#x} after the routine and {:#x} after \
                 its twin",
                routine.1[at], twin.1[at]
            );
        }
        self.tally.entry(name).or_default()[usize::from(routine.1 != before)] += 1;
    }

    /// Store-if-equal from `state`, expecting the word that is there and then one that is not.
    fn check_store_if_equal(&mut self, state: State) {
        // Markers of generations no state uses: `new` differs from every word, so a store
        // shows, and `other` equals none.
        let (new, other) = (marker(7), marker(8));
        for expected in [state.word, other] {
            // SAFETY: `new` is a marker, which keeps the slab invariant in any field.
            self.check(state, "store-if-equal", |r| unsafe {
                r.store_if_equal(state.field, expected, new)
            });
        }
    }

    /// Compares every routine with its twin from every scripted state.
    fn run(&mut self) {
        let object = NonNull::new(ptr::without_provenance_mut::<u8>(0xdead_0000)).unwrap();
        let second = NonNull::new(slot(1, 0)).unwrap();
        let no_magazine = [ptr::null_mut(), marker(0), marker(1), marker(u64::MAX)];
        for class in SmallClass::all() {
            let field = Field::hot(class);
            let slots = (0..class.magazine().slots()).map(|s| slot(0, s));
            for word in no_magazine.into_iter().chain(slots) {
                let link = filler(0);
                let state = State { field, word, link };
                self.check(state, "pop", |r| r.pop(class));
                self.check(state, "push", |r| r.push(class, object));
                self.check_store_if_equal(state);
            }
            for kind in [Spare::Full, Spare::Empty] {
                let field = Field::spare(class, kind);
                let empty = no_magazine.map(|word| (word, ptr::null_mut()));
                let stacks = [(slot(0, 0), ptr::null_mut()), (slot(0, 0), slot(1, 0))];
                for (word, link) in empty.into_iter().chain(stacks) {
                    let state = State { field, word, link };
                    self.check(state, "stack pop", |r| r.stack_pop(field));
                    if link.is_null() {
                        // SAFETY: the second magazine is this test's, in memory that is never
                        // unmapped, and in these states it is on no stack.
                        self.check(state, "stack push", |r| unsafe {
                            r.stack_push(field, second)
                        });
                    }
                    self.check_store_if_equal(state);
                }
            }
        }
    }
}

/// Pins the calling thread to the CPU it is on, and returns that CPU.
fn pin() -> Cpu {
    unsafe extern "C" {
        fn sched_setaffinity(pid: i32, len: usize, mask: *const u64) -> i32;
    }
    let cpu = Real
        .cpu()
        .expect("glibc registered rseq and the CPU has a slab");
    let mut mask = [0u64; SLAB_COUNT / 64];
    mask[cpu.index() / 64] = 1 << (cpu.index() % 64);
    // SAFETY: the call reads `len` bytes of `mask`, a live local of that size, and changes
    // only which CPUs this thread may run on.
    let status = unsafe { sched_setaffinity(0, size_of_val(&mask), mask.as_ptr()) };
    assert_eq!(status, 0, "pinning the thread to CPU {}", cpu.get());
    assert_eq!(Real.cpu(), Some(cpu));
    cpu
}

/// Run on its own by `every_routine_agrees_with_its_twin`; does nothing otherwise.
#[test]
fn child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let mut comparison = Comparison::new(pin());
    comparison.run();
    assert_eq!(Real.cpu(), Some(comparison.cpu), "the thread stayed pinned");
    // Two implementations that both did nothing would agree, so every routine must have
    // acted in some case and declined in another.
    for name in ["pop", "push", "store-if-equal", "stack push", "stack pop"] {
        let [declined, acted] = comparison.tally.get(name).copied().unwrap_or_default();
        assert!(
            declined > 0 && acted > 0,
            "{name} changed the state in {acted} cases and left it alone in {declined}"
        );
    }
    std::eprintln!("{DONE}");
}

#[test]
fn every_routine_agrees_with_its_twin() {
    // The test's name as the harness knows it: this module's path without the crate.
    let module = module_path!().split_once("::").map_or("", |(_, path)| path);
    // A twin that disagrees may fault (by following a marker, say); `ulimit -c 0` keeps that
    // from leaving a core dump behind.
    let output = Command::new("sh")
        .arg("-c")
        .arg("ulimit -c 0; exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", &std::format!("{module}::child"), "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && said.contains(DONE),
        "the comparison ended with {}: {said}",
        output.status
    );
}
