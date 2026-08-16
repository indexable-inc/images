# attrset-heavy: 10k-entry set built, then names sorted, then length
builtins.length (
  builtins.attrNames (
    builtins.listToAttrs (
      builtins.genList (i: {
        name = "attr${builtins.toString i}";
        value = i;
      }) 10000
    )
  )
)
