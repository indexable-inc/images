# Hermetic auth state: point the NIF at a throwaway HOME and scrub the OAuth
# client env before any NIF call, so the suite proves the signed-out boundary
# on any machine (a workstation with real credentials and a stored grant
# would otherwise flip `status` -- or worse, let `send` go live).
home = Path.join(System.tmp_dir!(), "gmail-ex-test-home-7620")
File.mkdir_p!(home)
System.put_env("HOME", home)
System.put_env("XDG_CONFIG_HOME", Path.join(home, ".config"))
System.delete_env("GOOGLE_OAUTH_CLIENT_ID")
System.delete_env("GOOGLE_OAUTH_CLIENT_SECRET")

ExUnit.start()
