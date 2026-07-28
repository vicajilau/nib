mod application;
mod config;
mod daemon;
mod i18n;
mod indicator;
mod ui;
mod window;

use application::NibApplication;
use gettextrs::LocaleCategory;
use gtk4::prelude::ApplicationExtManual;

/// Main entry point for the Nib host application.
///
/// Initializes logging, sets up internationalization (i18n) gettext domains,
/// instantiates the GTK4/Libadwaita application, and executes the event loop.
fn main() -> glib::ExitCode {
    // Set up logging
    tracing_subscriber::fmt::init();

    // Set up i18n
    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(config::GETTEXT_PACKAGE, config::LOCALEDIR)
        .expect("Failed to bind text domain");
    gettextrs::textdomain(config::GETTEXT_PACKAGE).expect("Failed to set text domain");

    tracing::info!("Starting Nib v{}", config::VERSION);

    let app = NibApplication::new();
    app.run()
}
