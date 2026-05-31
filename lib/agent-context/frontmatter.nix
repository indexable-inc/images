{ lib }:
/**
  Parse the leading YAML-ish frontmatter from a Markdown string.

  Handles the constrained shape this repository controls, not arbitrary YAML:
  a leading `---` fence, single-line `key: value` pairs, a closing `---`, then
  the body. The value is everything after the first `": "`, so colons inside a
  value (`description: Use X: do Y`) are preserved; a surrounding pair of single
  or double quotes is stripped. Multi-line block scalars are intentionally
  unsupported: keep each value on one line.

  Returns `{ frontmatter = { <key> = <value>; ... }; body = "<rest>"; }`. A
  string with no leading `---` yields empty `frontmatter` and the whole string
  as `body`.

  Arguments:
  - `text`: the raw Markdown contents of a section file.
*/
text:
let
  stripQuotes =
    raw:
    let
      s = lib.trim raw;
      len = lib.stringLength s;
      wrappedBy = q: lib.hasPrefix q s && lib.hasSuffix q s && len >= 2 * lib.stringLength q;
    in
    if wrappedBy "\"" || wrappedBy "'" then lib.substring 1 (len - 2) s else s;

  parsePair =
    line:
    let
      parts = lib.splitString ": " line;
    in
    if builtins.length parts < 2 then
      null
    else
      {
        name = lib.trim (builtins.head parts);
        value = stripQuotes (lib.concatStringsSep ": " (builtins.tail parts));
      };

  # Split the lines after the opening fence into the frontmatter block (up to
  # the closing `---`) and the remaining body lines.
  splitAtFence =
    let
      go =
        acc: remaining:
        if remaining == [ ] then
          {
            fm = acc;
            rest = [ ];
          }
        else if builtins.head remaining == "---" then
          {
            fm = acc;
            rest = builtins.tail remaining;
          }
        else
          go (acc ++ [ (builtins.head remaining) ]) (builtins.tail remaining);
    in
    go [ ];

  dropLeadingBlank =
    ls:
    if ls != [ ] && lib.trim (builtins.head ls) == "" then dropLeadingBlank (builtins.tail ls) else ls;

  lines = lib.splitString "\n" text;
  hasFrontmatter = lines != [ ] && builtins.head lines == "---";
in
if !hasFrontmatter then
  {
    frontmatter = { };
    body = text;
  }
else
  let
    split = splitAtFence (builtins.tail lines);
    pairs = builtins.filter (pair: pair != null) (map parsePair split.fm);
  in
  {
    frontmatter = lib.listToAttrs pairs;
    body = lib.concatStringsSep "\n" (dropLeadingBlank split.rest);
  }
