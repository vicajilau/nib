desktop_file = 'dev.victorcarreras.Nib.desktop'
install_data(desktop_file, install_dir: get_option('datadir') / 'applications')

desktop_file_validate = find_program('desktop-file-validate', required: false)
if desktop_file_validate.found()
  test('validate-desktop', desktop_file_validate, args: [files(desktop_file)])
endif

metainfo_file = 'dev.victorcarreras.Nib.metainfo.xml'
install_data(metainfo_file, install_dir: get_option('datadir') / 'metainfo')

appstreamcli = find_program('appstreamcli', required: false)
if appstreamcli.found()
  test('validate-metainfo', appstreamcli, args: ['validate', '--no-net', '--explain', files(metainfo_file)])
endif

install_data(
  'icons/hicolor/scalable/apps/dev.victorcarreras.Nib.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/scalable/apps',
)
