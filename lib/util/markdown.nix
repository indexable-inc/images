{lib}: let
  orderedKeys = order: attrs: let
    present = builtins.attrNames attrs;
  in
    (builtins.filter (key: builtins.elem key present) order)
    ++ lib.sort lib.lessThan (lib.subtractLists order present);

  renderValue = builtins.toJSON;

  renderFrontmatter = {
    frontmatter,
    order ? [],
  }:
    assert lib.assertMsg (builtins.isAttrs frontmatter)
    "markdown.renderFrontmatter: frontmatter must be an attrset"; let
      line = key: "${key}: ${renderValue frontmatter.${key}}";
    in
      lib.concatStringsSep "\n" (map line (orderedKeys order frontmatter));

  renderDocument = {
    frontmatter,
    content,
    order ? [],
  }:
    assert lib.assertMsg (builtins.isString content)
    "markdown.renderDocument: content must be a string"; ''
      ---
      ${renderFrontmatter {inherit frontmatter order;}}
      ---

      ${content}'';
in {
  inherit renderDocument renderFrontmatter;
}
