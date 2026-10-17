# One line per check: run SPEC CONFIG EXPECTATION. Sourced by ./check.

# 4. Depot stacks
run Depot Depot        ok
run Depot Depot_nover  violates OneOwner
run Depot Depot_wrap   violates OneOwner
