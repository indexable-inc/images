{
  id = "tui";
  inRustWorkspace = true;
  # Without this the crate has no CI gate at all (the same gap dashboard-core
  # closed): it compiles as a dependency of tui-py and the dashboard, but
  # nothing runs its tests and nothing runs clippy over it, so the PTY
  # manager, the frame sampler and the producer's send delivery were unlinted
  # on main.
  passthruTests = true;
}
