# One line per check: run SPEC CONFIG EXPECTATION. Sourced by ./check.
# SLOW=1 adds the checks that take more than a few minutes.

# 1. Restartable sequences and cids
run Rseq Rseq            ok
run Rseq Rseq_3t         ok
run Rseq Rseq_badcid     ok
run Rseq Rseq_norestart  violates SlabInvariant
run Rseq Rseq_sharedcid  violates SlabInvariant

# 2. Magazine exchange in the slow path
run Exchange Exchange  ok

# 3. Drain of an idle slab, and its repair after fork
run Drain Drain_v1              violates NoDouble
run Drain Drain_claim           ok
run Drain Drain_backout         ok
run Drain Drain_zerorule        violates Clean
run Drain Drain_inplace         violates NoDouble
run Drain Drain_sweep           violates NoLoss
run Drain Drain_sweep_dup       violates NoDouble
run Drain Drain_sweep_hold      ok
run Drain Drain_sweep_hold_any  violates NoDouble
run Drain Drain_gen             ok
run Drain Drain_gen_any         ok
run Drain Drain_gen_any_loss    violates NoLoss
# Both threads in every role: about 33 million states and ten minutes.
if [ -n "${SLOW:-}" ]; then run Drain Drain_gen_any_full ok; fi

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
run Summary Summary_run         ok
run Summary Summary_run_deep    ok
run Summary Summary_run_crash   ok
run Summary Summary_run_noreread violates Visible
# Three bits to a word: about 81 million states and six minutes.
if [ -n "${SLOW:-}" ]; then run Summary Summary_run_wide ok; fi

# 6. Purge as a claim
run Purge Purge               ok
run Purge Purge_straddle      ok
run Purge Purge_big           ok
run Purge Purge_claim_inside  violates PurgeSafe
run Purge Purge_zero_claimed  violates ZeroBitSound
run Purge Purge_zero_after    violates ZeroBitSound

# 7. Start-up
run Startup Startup             ok
run Startup Startup_fork        ok
run Startup Startup_tight       ok
run Startup Startup_tight_size  violates FullSize
run Startup Startup_tight_null  violates NoSpuriousNull

# 8. Fork
run Fork Fork               ok
run Fork Fork_summary       violates Visible
run Fork Fork_norepair      violates Clean
run Fork Fork_any           ok
run Fork Fork_any_loss      violates NoLoss
run Fork Fork_gen           ok
run Fork Fork_gen_any       ok
run Fork Fork_gen_any_loss  violates NoLoss
