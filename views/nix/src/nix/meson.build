project(
  'nix',
  'cpp',
  version : files('.version'),
  default_options : [
    'cpp_std=c++23',
    # TODO(Qyriad): increase the warning level
    'warning_level=1',
    'errorlogs=true', # Please print logs for tests that fail
    'localstatedir=/nix/var',
  ],
  meson_version : '>= 1.4',
  license : 'LGPL-2.1-or-later',
)

cxx = meson.get_compiler('cpp')

subdir('nix-meson-build-support/deps-lists')

nix_store = dependency('nix-store')

deps_private_maybe_subproject = [
  dependency('nix-util'),
  nix_store,
  dependency('nix-expr'),
  dependency('nix-flake'),
  dependency('nix-fetchers'),
  dependency('nix-main'),
  dependency('nix-cmd'),
]
deps_public_maybe_subproject = []
subdir('nix-meson-build-support/subprojects')

subdir('nix-meson-build-support/export-all-symbols')
subdir('nix-meson-build-support/windows-version')

configdata = configuration_data()

# The CLI has a more detailed version string than the libraries; see `nixVersion`
configdata.set_quoted('NIX_CLI_VERSION', meson.project_version())

fs = import('fs')
prefix = get_option('prefix')

bindir = get_option('bindir')
bindir = fs.is_absolute(bindir) ? bindir : prefix / bindir
configdata.set_quoted('NIX_BIN_DIR', bindir)

mandir = get_option('mandir')
mandir = fs.is_absolute(mandir) ? mandir : prefix / mandir
configdata.set_quoted('NIX_MAN_DIR', mandir)

config_priv_h = configure_file(
  configuration : configdata,
  output : 'cli-config-private.hh',
)

subdir('nix-meson-build-support/common')
subdir('nix-meson-build-support/generate-header')

nix_sources = [ config_priv_h ] + files(
  'add-to-store.cc',
  'app.cc',
  'build.cc',
  'bundle.cc',
  'cat.cc',
  'config-check.cc',
  'config.cc',
  'copy.cc',
  'crash-handler.cc',
  'derivation-add.cc',
  'derivation-show.cc',
  'derivation.cc',
  'develop.cc',
  'diff-closures.cc',
  'dump-path.cc',
  'edit.cc',
  'env.cc',
  'eval-persistent.cc',
  'eval.cc',
  'flake-prefetch-inputs.cc',
  'flake.cc',
  'formatter.cc',
  'hash.cc',
  'invocation-record.cc',
  'invocation.cc',
  'log.cc',
  'ls.cc',
  'main.cc',
  'make-content-addressed.cc',
  'man-pages.cc',
  'nar.cc',
  'optimise-store.cc',
  'path-from-hash-part.cc',
  'path-info.cc',
  'prefetch.cc',
  'profile.cc',
  'realisation.cc',
  'registry.cc',
  'repl.cc',
  'run.cc',
  'search.cc',
  'self-exe.cc',
  'sigs.cc',
  'store-builds.cc',
  'store-compress.cc',
  'store-copy-log.cc',
  'store-delete.cc',
  'store-gc.cc',
  'store-info.cc',
  'store-repair.cc',
  'store.cc',
  'upgrade-nix.cc',
  'verify.cc',
  'why-depends.cc',
)

if host_machine.system() != 'windows'
  nix_sources += files(
    'unix/daemon.cc',
    'unix/store-roots-daemon.cc',
  )
endif

nix_sources += [
  gen_header.process('doc/manual/generate-manpage.nix'),
  gen_header.process('doc/manual/generate-settings.nix'),
  gen_header.process('doc/manual/generate-store-info.nix'),
  gen_header.process('doc/manual/utils.nix'),
  gen_header.process('get-env.sh'),
  gen_header.process('profiles.md'),
  gen_header.process('help-stores.md'),
]

# The rest of the subdirectories aren't separate components,
# just source files in another directory, so we process them here.

build_remote_sources = files(
  'build-remote/build-remote.cc',
)
nix_build_sources = files(
  'nix-build/nix-build.cc',
)
nix_channel_sources = files(
  'nix-channel/nix-channel.cc',
)
unpack_channel_gen = gen_header.process('nix-channel/unpack-channel.nix')
nix_collect_garbage_sources = files(
  'nix-collect-garbage/nix-collect-garbage.cc',
)
nix_copy_closure_sources = files(
  'nix-copy-closure/nix-copy-closure.cc',
)
nix_env_buildenv_gen = gen_header.process('nix-env/buildenv.nix')
nix_env_sources = files(
  'nix-env/nix-env.cc',
  'nix-env/user-env.cc',
)
nix_instantiate_sources = files(
  'nix-instantiate/nix-instantiate.cc',
  'nix-instantiate/rust-eval.cc',
)
# The handle API's C++ side, shared by `nix eval` and nix-instantiate. Listed
# beside nix_sources rather than under nix-instantiate because it is not that
# command's any more.
rust_eval_session_sources = files('rust-eval-session.cc')
nix_store_sources = files(
  'nix-store/dotgraph.cc',
  'nix-store/graphml.cc',
  'nix-store/nix-store.cc',
)

# Hurray for Meson list flattening!
sources = [
  nix_sources,
  nix_build_sources,
  unpack_channel_gen,
  nix_collect_garbage_sources,
  nix_copy_closure_sources,
  nix_env_buildenv_gen,
  nix_env_sources,
  nix_instantiate_sources,
  nix_store_sources,
  rust_eval_session_sources,
]

if host_machine.system() != 'windows'
  sources += [
    build_remote_sources,
    nix_channel_sources,
  ]
endif

include_dirs = [ include_directories('.') ]

# The Rust evaluator links in as a staticlib built by cargo. Cargo owns the
# incremental story (its no-op rebuild is ~0.1s), so the target is always
# stale from meson's point of view and cargo decides what to do.
rust_eval = get_option('rust-eval')
rust_eval_prefix = ''
if rust_eval.enabled()
  rust_eval_prefix = get_option('rust-eval-prefix')
endif

if rust_eval.enabled() and rust_eval_prefix != ''
  # Prebuilt: another derivation compiled the archive and installed its header.
  # `include_directories()` is not used because meson rejects absolute paths
  # there, and this prefix is outside the source tree by construction.
  nix_eval_rs_lib = rust_eval_prefix / 'lib' / 'libnix_eval_rs.a'
  rust_eval_sources = []
  rust_eval_incdirs = []
  rust_eval_cargs = [ '-DHAVE_RUST_EVAL=1', '-I' + rust_eval_prefix / 'include' ]
elif rust_eval.enabled()
  cargo = find_program('cargo', required : true)
  rust_dir = meson.project_source_root() / '..' / '..' / 'rust'
  # `--target-dir` into the build directory rather than letting cargo default
  # to `<rust_dir>/target`, which is inside the source tree. This is what an
  # out-of-tree meson build is supposed to do anyway, and it is required for a
  # packaged build: a Nix derivation unpacks the source read-only apart from
  # the work directory, so the default made cargo die with
  # `Permission denied (os error 13) at path ".../source/rust/target..."`
  # before it compiled anything. Incrementality is unaffected -- cargo still
  # owns it, just keyed to a build directory instead of the source.
  cargo_target_dir = meson.current_build_dir() / 'rust-target'
  nix_eval_rs = custom_target(
    'nix-eval-rs',
    output : 'libnix_eval_rs.a',
    command : [
      'sh',
      '-c',
      '@0@ build --release --manifest-path @1@/Cargo.toml -p nix-eval-rs --target-dir @3@ @2@ && cp @3@/release/libnix_eval_rs.a "$1"'.format(
        cargo.full_path(),
        rust_dir,
        get_option('rust-eval-cargo-features'),
        cargo_target_dir,
      ),
      'ignored',
      '@OUTPUT@',
    ],
    build_always_stale : true,
  )
  nix_eval_rs_lib = nix_eval_rs.full_path()
  # A custom_target must be declared as a source so ninja orders the link
  # after it. A prebuilt archive already exists, so it needs no ordering.
  rust_eval_sources = [ nix_eval_rs ]
  rust_eval_incdirs = include_directories(
    '..' / '..' / 'rust' / 'nix-eval-rs' / 'include',
  )
  rust_eval_cargs = [ '-DHAVE_RUST_EVAL=1' ]
endif

if rust_eval.enabled()
  deps_other += declare_dependency(
    sources : rust_eval_sources,
    # link_with cannot take a custom_target; pass the archive as link args.
    link_args : [ nix_eval_rs_lib ],
    include_directories : rust_eval_incdirs,
    compile_args : rust_eval_cargs,
  )
endif

# The phantom-token guard. Its own executable because it needs both the
# command layer's constants and the evaluator's ABI vocabulary linked, and the
# Rust archive is only pulled into targets that ask for it.
if rust_eval.enabled()
  refusal_vocabulary_test = executable(
    'nix-refusal-vocabulary-test',
    files('rust-eval-refusal-vocabulary-test.cc'),
    dependencies : deps_private_subproject + deps_private + deps_other,
    include_directories : include_dirs,
  )
  test(
    'refusal-vocabulary',
    refusal_vocabulary_test,
    suite : 'nix',
    protocol : 'exitcode',
  )
endif

this_exe = executable(
  meson.project_name(),
  sources,
  dependencies : deps_private_subproject + deps_private + deps_other,
  include_directories : include_dirs,
  link_args : linker_export_flags,
  install : true,
  cpp_pch : do_pch ? [ 'pch/precompiled-headers.hh' ] : [],
)

meson.override_find_program('nix', this_exe)

nix_symlinks = [
  'nix-build',
  'nix-channel',
  'nix-collect-garbage',
  'nix-copy-closure',
  'nix-daemon',
  'nix-env',
  'nix-hash',
  'nix-instantiate',
  'nix-prefetch-url',
  'nix-shell',
  'nix-store',
]

executable_suffix = ''
if host_machine.system() == 'windows'
  executable_suffix = '.exe'
endif

nix_symlinks_targets = []
foreach linkname : nix_symlinks
  install_symlink(
    linkname + executable_suffix,
    # TODO(Qyriad): should these continue to be relative symlinks?
    pointing_to : fs.name(this_exe),
    install_dir : get_option('bindir'),
    # The 'runtime' tag is what executables default to, which we want to emulate here.
    install_tag : 'runtime',
  )
  symlink_target = custom_target(
    command : [ 'ln', '-sf', fs.name(this_exe), '@OUTPUT@' ],
    output : linkname + executable_suffix,
    # native doesn't allow dangling symlinks, but the target executable often doesn't exist at this time
    env : {'MSYS' : 'winsymlinks:lnk'},
    # TODO(Ericson2314): Don't do this once we have the `meson.override_find_program` working)
    build_by_default : true,
    depends : this_exe,
  )
  # TODO(Ericson3214): Doesn't yet work
  #meson.override_find_program(linkname, t)
  nix_symlinks_targets += symlink_target
endforeach

install_symlink(
  'build-remote',
  pointing_to : '..' / '..' / get_option('bindir') / fs.name(this_exe),
  install_dir : get_option('libexecdir') / fs.name(this_exe),
  # The 'runtime' tag is what executables default to, which we want to emulate here.
  install_tag : 'runtime',
)

custom_target(
  command : [ 'ln', '-sf', fs.name(this_exe), '@OUTPUT@' ],
  output : 'build-remote' + executable_suffix,
  # native doesn't allow dangling symlinks, but the target executable often doesn't exist at this time
  env : {'MSYS' : 'winsymlinks:lnk'},
  # TODO(Ericson2314): Don't do this once we have the `meson.override_find_program` working)
  build_by_default : true,
  depends : this_exe,
)
# TODO(Ericson3214): Doesn't yet work
#meson.override_find_program(linkname, t)

localstatedir = nix_store.get_variable(
  'localstatedir',
  default_value : get_option('localstatedir'),
)
assert(localstatedir == get_option('localstatedir'))
store_dir = nix_store.get_variable('storedir')
subdir('scripts')
subdir('misc')
