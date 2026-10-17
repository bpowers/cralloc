# One line per check: run SPEC CONFIG EXPECTATION. Sourced by ./check.

# 1. Restartable sequences and cids
run Rseq Rseq            ok
run Rseq Rseq_3t         ok
run Rseq Rseq_norestart  violates SlabInvariant
run Rseq Rseq_sharedcid  violates SlabInvariant

# 4. Depot stacks
run Depot Depot        ok
run Depot Depot_nover  violates OneOwner
run Depot Depot_wrap   violates OneOwner
