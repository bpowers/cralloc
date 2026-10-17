import Asmalloc.TCB.Axioms
import Asmalloc.Proof.Examples.X86_64.Fs

/-!
# The examples' axioms

The examples under `Proof/Examples/` are not artifacts, so the emitter does
not audit them: this file does, as it is not a module and sees their proofs.
-/

#assert_standard_axioms VG.Proof.Examples.X86_64.Fs.storeThenLoad_ok
