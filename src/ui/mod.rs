pub mod display_settings;
pub mod status_page;

use gtk4::glib;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use display_settings::DisplaySettingsView;
use status_page::ConnectionStatusPage;

/// GObject subclass implementation details for `NibView`.
mod imp {
    use super::*;

    /// Private instance state backing the `NibView` GObject subclass.
    #[derive(Default)]
    pub struct NibView {
        /// View stack switching between the connection page and the settings page.
        pub stack: adw::ViewStack,
        /// Page listing connected devices and controlling their active streams.
        pub status_page: ConnectionStatusPage,
        /// Page exposing display mode, resolution, framerate, and stylus settings.
        pub settings_view: DisplaySettingsView,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NibView {
        const NAME: &'static str = "NibView";
        type Type = super::NibView;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for NibView {
        /// Builds the view layout once the GObject instance has finished construction.
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_layout();
        }
    }

    impl WidgetImpl for NibView {}
    impl BoxImpl for NibView {}
}

glib::wrapper! {
    /// Root content view combining the connection status page and display settings page
    /// inside a Libadwaita view stack.
    pub struct NibView(ObjectSubclass<imp::NibView>)
        @extends gtk4::Widget, gtk4::Box,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl NibView {
    /// Instantiates a new `NibView`.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Wires the settings page's config provider into the status page and populates the
    /// view stack with the connection and settings pages.
    fn setup_layout(&self) {
        let imp = self.imp();
        self.set_orientation(gtk4::Orientation::Vertical);

        let settings = imp.settings_view.clone();
        imp.status_page
            .set_config_provider(move || settings.get_stream_config());

        imp.stack.add_titled_with_icon(
            &imp.status_page,
            Some("connect"),
            "Connection",
            "video-display-symbolic",
        );

        imp.stack.add_titled_with_icon(
            &imp.settings_view,
            Some("settings"),
            "Display Settings",
            "display-symbolic",
        );

        self.append(&imp.stack);
    }

    /// Stops any currently active device stream, delegating to the status page.
    pub fn stop_active_stream(&self) {
        self.imp().status_page.stop_active_stream();
    }

    /// Returns whether any device stream is currently active.
    pub fn is_streaming(&self) -> bool {
        self.imp().status_page.is_streaming()
    }

    /// Toggles streaming for the first known connected device.
    pub fn toggle_stream(&self) {
        self.imp().status_page.toggle_stream();
    }

    /// Toggles streaming for the device identified by the given ADB serial.
    pub fn toggle_stream_for_device(&self, serial: &str) {
        self.imp().status_page.toggle_stream_for_device(serial);
    }
}

impl Default for NibView {
    fn default() -> Self {
        Self::new()
    }
}
