import Cralloc.TCB.Rust

/-!
# Routines that verify and are wrong, which the emitter refuses

Each file here is a routine with a whole proof, that a review found would run wrong: what the proof
is of is not what the machine does. The model cannot see why, so the emitter refuses each
(`Rust.checkRseq`), and these keep it refusing them.
-/

namespace VG.Test.Forgery

/-- Whether the emitter refuses the artifact, saying `why`. -/
def refusedFor (a : Artifact) (why : String) : Bool :=
  match Rust.files [a] with
  | .error e => (e.splitOn why).length > 1
  | .ok _ => false

end VG.Test.Forgery
