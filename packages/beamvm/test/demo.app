%% OTP application resource for the hot-reload demo (see demo-v1.ex). Written
%% by hand because the demo is compiled with bare elixirc, not mix, which
%% would otherwise generate this from mix.exs.
{application, demo, [
  {description, "beamvm hot-reload demo"},
  {vsn, "1.0.0"},
  {modules, ['Elixir.Demo.App', 'Elixir.Demo.Server']},
  {registered, []},
  {applications, [kernel, stdlib, elixir]},
  {mod, {'Elixir.Demo.App', []}}
]}.
