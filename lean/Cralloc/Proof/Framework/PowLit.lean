module

/-!
# Numeral exponents are natural numbers

This only changes how terms are elaborated, not what they mean.

In `x ^ 26` the type of the numeral `26` is only fixed by default instances,
which Lean tries last, once for every numeral still pending in the whole
term: elaborating a statement with many powers of numerals (`h0 < 2 ^ 26 → … →
x = (y + z / 2 ^ 26) % 2 ^ 64`) takes time quadratic in their number, up to
seconds for one theorem header. Here `x ^ 26` is elaborated as `x ^ (26 : Nat)`,
without the search: the exponent's default type is `Nat`, so this is the same
term.

The macro is global: every module that imports this one (most of `Proof/`,
through `GetElem`) elaborates numeral exponents this way. `Spec/`, `Impl/`
and `TCB/` never import `Proof/`, so their terms are elaborated as before.
-/

public meta section

/-- A numeral exponent is a `Nat`. -/
macro_rules | `($x ^ $n:num) => `($x ^ ($n : Nat))
