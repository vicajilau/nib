project('nib', 'rust',
          version: '0.1.0',
    meson_version: '>= 0.59.0',
  default_options: [ 'warning_level=2', 'werror=false' ],
)

i18n = import('i18n')
gnome = import('gnome')

dependency('gtk4', version: '>= 4.12.0')
dependency('libadwaita-1', version: '>= 1.5.0')
dependency('gstreamer-1.0', version: '>= 1.20.0')

cargo = find_program('cargo', required: true)

custom_target('cargo-build',
  build_by_default: true,
  output: 'nib',
  console: true,
  command: [
    cargo, 'build', '--manifest-path', meson.project_source_root() / 'Cargo.toml',
  ],
)
