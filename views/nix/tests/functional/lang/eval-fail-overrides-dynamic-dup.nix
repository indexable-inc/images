# A dynamic attribute cannot re-use a name `__overrides` just introduced.
# The order is what makes this an error rather than a silent last-wins:
# `ExprAttrs::eval` applies the override set first (eval.cc:1465-1471) and
# only then inserts the dynamic attributes (eval.cc:1489), so by the time
# `${key}` is inserted the name is already present and the duplicate check
# fires. Reported against the *override's* definition site, since that is
# the position the existing binding carries.
let
  overrides = {
    dyn = "from overrides";
  };
  key = "dyn";
in
(rec {
  __overrides = overrides;
  ${key} = "from the rec set";
})
