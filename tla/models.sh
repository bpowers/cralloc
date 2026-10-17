# One line per check: run SPEC CONFIG EXPECTATION. Sourced by ./check.

# 1. Restartable sequences and cids
run Rseq Rseq            ok
run Rseq Rseq_3t         ok
run Rseq Rseq_norestart  violates SlabInvariant
run Rseq Rseq_sharedcid  violates SlabInvariant

# 3. Drain of an idle slab, and its repair after fork
run Drain Drain_v1              violates NoDouble
run Drain Drain_claim           ok
run Drain Drain_claim_2m        ok
run Drain Drain_zerorule        violates Clean
run Drain Drain_sweep           violates NoLoss
run Drain Drain_sweep_hold      ok
run Drain Drain_sweep_hold_any  violates NoDouble
run Drain Drain_gen             ok
run Drain Drain_gen_any         ok
run Drain Drain_gen_any_loss    violates NoLoss

# 4. Depot stacks
run Depot Depot        ok
run Depot Depot_nover  violates OneOwner
run Depot Depot_wrap   violates OneOwner

# 5. Summary bits over the free bitmap
run Summary Summary             ok
run Summary Summary_deep        ok
run Summary Summary_noreread    violates Visible
run Summary Summary_crash       violates Healed
run Summary Summary_heal        ok
run Summary Summary_heal_deep   ok
run Summary Summary_heal_crash  ok
