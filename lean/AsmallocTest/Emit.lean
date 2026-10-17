import Asmalloc.TCB.Emit

/-!
# Golden tests for the emitter's driver and arguments

These tests pin down which list of artifacts each registration file names
and how the emitter reads its arguments.
-/

namespace VG.Test.Emit

open VG.Emit

#guard listOf "Asmalloc.Artifacts.Selftest.X86_64" == "VG.Artifacts.Selftest.X86_64.artifacts"

-- The output directory is `generated` unless one is named; anything else is
-- a usage error.
/-- `parseArgs`, with the directory as a string. -/
def parsed (args : List String) : Option (Bool × String) :=
  (parseArgs args).map fun (c, d) => (c, d.toString)

#guard parsed [] == some (false, "generated")
#guard parsed ["--check"] == some (true, "generated")
#guard parsed ["../crates/asmalloc/src/asm"] == some (false, "../crates/asmalloc/src/asm")
#guard parsed ["--check", "out"] == some (true, "out")
#guard parseArgs ["out", "--check"] == none
#guard parseArgs [""] == none
#guard parseArgs ["--check", ""] == none
#guard parseArgs ["--part", "f"] == none
#guard parseArgs ["a", "b"] == none

end VG.Test.Emit
