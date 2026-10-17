module

public import Asmalloc.Proof.Framework.PowLit
public meta import Asmalloc.Proof.Framework.PowLit
public import Asmalloc.Proof.Framework.Omega
public meta import Asmalloc.Proof.Framework.Omega

/-!
# Index bounds without rewriting every hypothesis

Untrusted: this only changes how proofs are found.

`xs[i]` proves `i < n` with `get_elem_tactic`, which after `assumption` tries
the rules of `get_elem_tactic_extensible`, the most recent first. The ones
of Lean core rewrite every hypothesis (for the polymorphic ranges) and then
run `omega`: in a proof with many hypotheses, a tenth of a second or more for
every `v[1]` of a hash value, which is most of the time of some proofs. The
rules here are tried before them, in this order:

* a literal index is in bounds by `decide` (for any other index `decide`
  fails at once);
* an index `i : Fin n` (`xs[i]`, `xs[i.val]`) is in bounds by `i.isLt`, when
  the length is `n` up to reducible unfolding;
* any other index by `omega` on the arithmetic facts connected to the bound
  (`omega_arith_once`), e.g. `v[k + 8]` from `k < 8`.

If they fail, core's rules run as before.
-/

public meta section

-- The most recent rule is tried first: these run in the reverse order.
macro_rules | `(tactic| get_elem_tactic_extensible) => `(tactic| omega_arith_once)
macro_rules | `(tactic| get_elem_tactic_extensible) => `(tactic| with_reducible exact Fin.isLt _)
macro_rules | `(tactic| get_elem_tactic_extensible) => `(tactic| decide)
