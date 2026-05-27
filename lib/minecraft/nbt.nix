let
  tagged = tag: value: {
    __minecraftNbt = tag;
    inherit value;
  };
in
{
  root = name: value: {
    __minecraftNbt = "root";
    inherit name value;
  };
  byte = tagged "byte";
  short = tagged "short";
  int = tagged "int";
  long = tagged "long";
  float = tagged "float";
  double = tagged "double";
  string = tagged "string";
  bool = value: tagged "byte" (if value then 1 else 0);
  byteArray = tagged "byteArray";
  intArray = tagged "intArray";
  longArray = tagged "longArray";
  list = tagged "list";
  compound = tagged "compound";
}
