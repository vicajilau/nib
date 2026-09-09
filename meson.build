project('nib', 'rust',
          version: '0.1.3',
    meson_version: '>= 0.59.0',
  default_options: [ 'warning_level=2', 'werror=false' ],
)

gnome = import('gnome')

dependency('gtk4', version: '>= 4.12.0')
dependency('libadwaita-1', version: '>= 1.5.0')
dependency('gstreamer-1.0', version: '>= 1.20.0')

cargo = find_program('cargo', required: true)
sh = find_program('sh', required: true)

cargo_target_dir = meson.project_build_root() / 'cargo-target'
rust_target = get_option('buildtype') == 'release' ? 'release' : 'debug'

cargo_build_cmd = '"@0@" build --manifest-path "@1@" --target-dir "@2@"'.format(
  cargo.full_path(),
  meson.project_source_root() / 'Cargo.toml',
  cargo_target_dir,
)
if rust_target == 'release'
  cargo_build_cmd += ' --release'
endif
cargo_build_cmd += ' && cp "@0@" "$1"'.format(cargo_target_dir / rust_target / 'nib')

custom_target('cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'nib',
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [sh, '-c', cargo_build_cmd, '_', '@OUTPUT@'],
)

subdir('data')

gnome.post_install(
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)
