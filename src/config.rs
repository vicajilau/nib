/// Freedesktop application ID used for GTK/GIO application registration and the About dialog.
pub const APP_ID: &str = "dev.victorcarreras.Nib";
/// Gettext translation domain name used to look up compiled `.mo` locale files.
pub const GETTEXT_PACKAGE: &str = "nib";
/// Filesystem path where compiled gettext locale files are installed.
pub const LOCALEDIR: &str = "/usr/share/locale";
/// Application version string, taken from the Cargo package version at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
