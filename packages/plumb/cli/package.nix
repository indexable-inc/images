{
  id = "plumb";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  mirror = {
    repo = "indexable-inc/plumb";
    description = "An inspectable bash-subset shell, library-first: every run is a value, pipe stages are captured, outputs auto-bind to variables.";
    topics = [
      "shell"
      "bash"
      "repl"
      "rust"
      "llm"
      "ix"
    ];
  };
  passthruTests = true;
}
