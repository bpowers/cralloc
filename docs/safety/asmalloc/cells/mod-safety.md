# Safety ledger: `crates/asmalloc/src/cells/mod.rs`

`Cells<T, N>` holds `[T; N]` in an `UnsafeCell` so that Miri sees one interior-mutable range
where it would see `N` (the module documentation says why). Its accessor `get` is proved in
bounds by Kani (`cells::verify::every_index_names_an_entry`); this entry covers the one other
`unsafe` site, the `Sync` impl.

**What a `Cells` offers.** Its field is private to the module. The module makes the cell once,
in `new`, from an owned array, and has one method that reads it, `get(&self, index) -> &T`,
which returns a shared reference to the entry at `index & (N - 1)`. Nothing in the module
makes a `&mut [T; N]`, a `&mut T`, or writes through the cell's pointer, and nothing outside
can, so every reference to the contents is a shared reference to one entry. (`&mut Cells`,
from an owner with exclusive access, reaches nothing more: there is no method that takes it.)

## CELLS-01: `unsafe impl<T: Sync, const N: usize> Sync for Cells<T, N>`

**Obligation.** `Sync` for `Cells<T, N>` means that a `&Cells<T, N>` may be used from several
threads at once. `UnsafeCell` is not `Sync`, so the compiler does not derive it.

**Proof.** What a thread can do with a `&Cells<T, N>` is call `get` and use the `&T` it
returns; by the paragraph above, nothing else. Several threads holding `&Cells` therefore hold
`&T`s to entries of the array, and nothing else that reaches it, which is exactly what several
threads holding a `&[T; N]` can do, and that is sound when `T: Sync` (`[T; N]: Sync` exactly
when `T: Sync`). The impl requires `T: Sync`. Moving a `Cells` to another thread (`Send`) is
the compiler's own impl, which `UnsafeCell<[T; N]>` has when `T: Send`.

The one difference from a plain array, that the entries are reached through `UnsafeCell::get`'s
raw pointer, changes no thread's view: a shared reference made from it is a shared reference
like any other, and the entries the tables hold (`AtomicU8`, `AtomicUsize`, the back end's
atomic words and headers made of them) are accessed only through their own atomic operations.
