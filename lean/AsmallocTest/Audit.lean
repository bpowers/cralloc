import Asmalloc.TCB.Audit

/-!
# Tests for the environment audit

`#assert_no_compiler_overrides` rejects code that the compiler would run in
place of what the kernel checked, wherever it is among what its roots use
(here, in this module), and `#assert_spec_origin` rejects a `VG.Spec`
definition declared outside `Spec/` (here, in this module).
-/

-- `#guard_msgs` compares every message of its command, which would include
-- the lakefile's profile of it (`ci/lean_profile.py`), different on every
-- machine.
set_option profiler false

namespace VG.Test.Audit

def two : Nat := 2

def three : Nat := 3

/-- The kernel sees `2`; compiled code returns `3`. -/
@[implemented_by three] def swapped : Nat := 2

def usesSwapped : Nat := swapped + two

/- Types and proofs are erased from compiled code: what a theorem states or
its proof uses is not what compiled code runs. -/
theorem swapped_eq : swapped = 2 := rfl

-- `swapped`'s override is found through `usesSwapped`.
/-- error: the compiled code of [VG.Test.Audit.usesSwapped] is not what the kernel checked:
@[implemented_by] on VG.Test.Audit.swapped -/
#guard_msgs in
#assert_no_compiler_overrides usesSwapped

-- Neither `two` nor `swapped_eq` runs `swapped`.
/-- info: the compiled code of [VG.Test.Audit.two, VG.Test.Audit.swapped_eq] is what the kernel checked -/
#guard_msgs in
#assert_no_compiler_overrides two swapped_eq

end VG.Test.Audit

/-- A contract that `Spec/` never declared. -/
def VG.Spec.Fake.fooContract : Nat := 0

/-- A theorem in the namespace is not a definition anything could use as a
specification. -/
theorem VG.Spec.Fake.foo_eq : VG.Spec.Fake.fooContract = 0 := rfl

/-- error: these `VG.Spec` names are not declared in `Spec/`:
VG.Spec.Fake.fooContract (in AsmallocTest.Audit) -/
#guard_msgs in
#assert_spec_origin
