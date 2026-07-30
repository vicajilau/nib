//! Nib streams an Android device's display to the desktop as a GNOME virtual monitor (or
//! mirrors an existing one), over PipeWire/GStreamer for video and a binary TCP protocol for
//! touch/stylus input, both tunneled through `adb reverse`. See `daemon` for the streaming
//! pipeline and `ui`/`window` for the GTK4/Libadwaita front end.

mod application;
mod config;
mod daemon;
mod i18n;
mod indicator;
mod ui;
mod window;

use application::NibApplication;
use gtk4::prelude::ApplicationExtManual;

/// Main entry point for the Nib host application.
///
/// Initializes logging, instantiates the GTK4/Libadwaita application, and executes the event
/// loop. UI string translation is handled by `crate::i18n`, not gettext (see its doc comment).
fn main() -> glib::ExitCode {
    // Set up logging
    tracing_subscriber::fmt::init();

    tracing::info!("Starting Nib v{}", config::VERSION);

    let app = NibApplication::new();
    app.run()
}
