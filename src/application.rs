use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::config;
use crate::window::NibWindow;

use crate::indicator::IndicatorManager;
use std::cell::RefCell;
use std::sync::Arc;

/// GObject subclass implementation details for `NibApplication`.
mod imp {
    use super::*;

    /// Private instance state backing the `NibApplication` GObject subclass.
    #[derive(Default)]
    pub struct NibApplication {
        /// Shared handle to the running system tray indicator, set up once at startup.
        pub indicator: RefCell<Option<Arc<IndicatorManager>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NibApplication {
        const NAME: &'static str = "NibApplication";
        type Type = super::NibApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for NibApplication {}

    impl ApplicationImpl for NibApplication {
        /// Presents the existing main window, or creates and presents a new one on first activation.
        fn activate(&self) {
            let app = self.obj();
            if let Some(window) = app.active_window() {
                window.present();
                return;
            }

            let window = NibWindow::new(&app);
            window.present();
        }

        /// Runs once before activation to register global actions and start the tray indicator.
        fn startup(&self) {
            self.parent_startup();
            let app = self.obj();
            app.setup_actions();
            app.setup_indicator();
        }
    }

    impl GtkApplicationImpl for NibApplication {}
    impl AdwApplicationImpl for NibApplication {}
}

glib::wrapper! {
    /// Main GTK4 / Libadwaita application wrapper managing system tray indicators, global actions, and active windows.
    pub struct NibApplication(ObjectSubclass<imp::NibApplication>)
        @extends gio::Application, gtk4::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl NibApplication {
    /// Instantiates a new `NibApplication` configured with the application ID.
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::NON_UNIQUE)
            .build()
    }

    /// Sets up global keyboard shortcuts and GIO application actions (`app.quit`, `app.about`, `app.toggle-stream`).
    fn setup_actions(&self) {
        let quit_action = gio::SimpleAction::new("quit", None);
        let app = self.clone();
        quit_action.connect_activate(move |_, _| {
            app.quit();
        });
        self.add_action(&quit_action);

        let about_action = gio::SimpleAction::new("about", None);
        let app = self.clone();
        about_action.connect_activate(move |_, _| {
            app.show_about();
        });
        self.add_action(&about_action);

        let toggle_stream_action = gio::SimpleAction::new("toggle-stream", None);
        let app = self.clone();
        toggle_stream_action.connect_activate(move |_, _| {
            app.toggle_stream();
        });
        self.add_action(&toggle_stream_action);

        self.set_accels_for_action("app.quit", &["<Control>q"]);
    }

    /// Initializes the system tray status indicator and registers action callbacks.
    fn setup_indicator(&self) {
        let show_cb = Arc::new(|| {
            glib::idle_add_once(|| {
                if let Some(app) = gio::Application::default() {
                    app.activate();
                }
            });
        });

        let toggle_device_cb = Arc::new(|serial: String| {
            glib::idle_add_once(move || {
                if let Some(app) = gio::Application::default() {
                    if let Ok(nib_app) = app.downcast::<NibApplication>() {
                        nib_app.toggle_stream_for_device(&serial);
                    }
                }
            });
        });

        let quit_cb = Arc::new(|| {
            glib::idle_add_once(|| {
                if let Some(app) = gio::Application::default() {
                    app.quit();
                }
            });
        });

        if let Some(mgr) = IndicatorManager::new(show_cb, toggle_device_cb, quit_cb) {
            self.imp().indicator.replace(Some(Arc::new(mgr)));
        }
    }

    /// Returns an atomic handle to the system tray indicator manager if active.
    pub fn indicator(&self) -> Option<Arc<IndicatorManager>> {
        self.imp().indicator.borrow().clone()
    }

    /// Toggles the video display stream for a specific connected device identified by serial.
    pub fn toggle_stream_for_device(&self, serial: &str) {
        if let Some(window) = self.active_window() {
            if let Ok(nib_win) = window.downcast::<NibWindow>() {
                nib_win.toggle_stream_for_device(serial);
            }
        }
    }

    /// Toggles active display streaming for the first available connected device.
    pub fn toggle_stream(&self) {
        if let Some(window) = self.active_window() {
            if let Ok(nib_win) = window.downcast::<NibWindow>() {
                nib_win.toggle_stream();
            }
        } else {
            self.activate();
            if let Some(window) = self.active_window() {
                if let Ok(nib_win) = window.downcast::<NibWindow>() {
                    nib_win.toggle_stream();
                }
            }
        }
    }

    /// Displays the native Libadwaita About dialog.
    fn show_about(&self) {
        let Some(window) = self.active_window() else {
            tracing::warn!("show_about: no active window to attach the About dialog to");
            return;
        };
        let dialog = adw::AboutDialog::builder()
            .application_name("Nib")
            .application_icon(config::APP_ID)
            .developer_name("Vicajilau")
            .version(config::VERSION)
            .comments(
                "Connect your Android tablet as an external display with touch and stylus support",
            )
            .website("https://github.com/vicajilau/nib")
            .issue_url("https://github.com/vicajilau/nib/issues")
            .license_type(gtk4::License::Gpl30)
            .translator_credits("Vicajilau")
            .build();

        dialog.present(Some(&window));
    }
}

impl Default for NibApplication {
    fn default() -> Self {
        Self::new()
    }
}
