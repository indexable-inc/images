cxx = meson.get_compiler('cpp')

deps_other = []
subdir('nix-meson-build-support/common/asan-options')

libstoreconsumer_tester = executable(
  'test-libstoreconsumer',
  'main.cc',
  dependencies : deps_other + [
    dependency('nix-store'),
  ],
  build_by_default : false,
)

register_realisation_tester = executable(
  'test-register-realisation',
  'register-realisation.cc',
  dependencies : deps_other + [
    dependency('nix-store'),
  ],
  build_by_default : false,
)
