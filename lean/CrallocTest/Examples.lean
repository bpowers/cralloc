import Cralloc.TCB.Axioms
import Cralloc.Proof.Examples.X86_64.Fs
import Cralloc.Proof.Examples.X86_64.Lea
import Cralloc.Proof.Examples.X86_64.Branch
import Cralloc.Proof.Examples.X86_64.Load32

/-!
# The examples' axioms

The examples under `Proof/Examples/` are not artifacts, so the emitter does
not audit them: this file does, as it is not a module and sees their proofs.
-/

#assert_standard_axioms VG.Proof.Examples.X86_64.Fs.storeThenLoad_ok
#assert_standard_axioms VG.Proof.Examples.X86_64.Lea.addrs_ok
#assert_standard_axioms VG.Proof.Examples.X86_64.Branch.check_ok
#assert_standard_axioms VG.Proof.Examples.X86_64.Branch.cmpInc_ok
#assert_standard_axioms VG.Proof.Examples.X86_64.Load32.cidOffset_ok
