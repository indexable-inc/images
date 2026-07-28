{
  id = "dashboard-core";
  inRustWorkspace = true;
  # Without this the crate has no CI gate at all: it compiles as a dependency of
  # `dashboard`, but nothing runs its tests and nothing runs clippy over it, so
  # the hub, the HTTP surface and the websocket protocol were unlinted on main.
  passthruTests = true;
}
