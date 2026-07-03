{
  btop,
  ix,
}:
btop.overrideAttrs (old: {
  src = ix.btopSrc;

  meta =
    old.meta
    // {
      homepage = "https://github.com/indexable-inc/btop";
    };
})
