# `__overrides` against the things a rec set can do with a name, in one
# expression, because each is a separate branch of `ExprAttrs::eval`
# (eval.cc:1455-1489) and a backend can get any of them right on its own.
let

  overrides = {
    # (1) A name the rec set defines. Replaces both the attribute and the
    #     scope slot, so `usesA` below reads 2 and not 1.
    a = 2;
    # (2) A name the rec set does not define. Appended to the attribute set,
    #     and NOT put in scope: `ExprAttrs::eval` writes `env2` only for the
    #     names it found among the rec bindings, so `usesFresh` still sees
    #     the `fresh` from the enclosing `let`.
    fresh = "from overrides";
  };

  fresh = "from let";

  key = "dyn";

in
(rec {
  __overrides = overrides;
  a = 1;
  usesA = a;
  usesFresh = fresh;
  # (3) A dynamic attribute, applied after the override (eval.cc:1489) and
  #     therefore onto the overridden set. Not colliding with an override
  #     name here; that case is a failure, and it is
  #     eval-fail-overrides-dynamic-dup.nix.
  ${key} = "dynamic";
  b = 3;
})
