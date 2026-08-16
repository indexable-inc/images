# `~/...` is `getHome()` and the rest of the literal concatenated
# (parser.y:465), so every property below is a property of that one string.
# Written as relations between home paths rather than against a literal
# `$HOME`, so the expected output does not depend on who runs it.
let
  p = ~/dir/file.txt;
in
{
  # It is a path, not a string -- the literal produces an ExprPath.
  isPath = builtins.isPath p;
  # The tail is what followed the `~`.
  base = baseNameOf p;
  parent = baseNameOf (dirOf p);
  # Absolute, and rooted at the same place as any other `~` literal, so the
  # home directory is resolved once per literal to the same answer.
  sameRoot = dirOf (dirOf p) == dirOf ~/dir;
  # `~` is not relative to the file being evaluated: a `./` literal in this
  # same file lands somewhere else entirely.
  notRelative = p != ./dir/file.txt;
  # A path carries no string context, so it can be appended to.
  appended = baseNameOf (p + "/x");
}
