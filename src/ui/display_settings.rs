use crate::daemon::virtual_monitor::DisplayMode;
use crate::daemon::StreamConfig;
use crate::i18n;
use gtk4::glib;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// GObject subclass implementation details for `DisplaySettingsView`.
mod imp {
    use super::*;

    /// Private instance state backing the `DisplaySettingsView` GObject subclass.
    #[derive(Default)]
    pub struct DisplaySettingsView {
        pub preferences_group: adw::PreferencesGroup,
        /// Selects between extending the desktop and mirroring the primary display.
        pub mode_row: adw::ComboRow,
        /// Selects the virtual/mirrored display resolution.
        pub resolution_row: adw::ComboRow,
        /// Selects the target stream framerate.
        pub framerate_row: adw::ComboRow,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DisplaySettingsView {
        const NAME: &'static str = "DisplaySettingsView";
        type Type = super::DisplaySettingsView;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for DisplaySettingsView {
        /// Builds the settings form once the GObject instance has finished construction.
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_ui();
        }
    }

    impl WidgetImpl for DisplaySettingsView {}
    impl BoxImpl for DisplaySettingsView {}
}

glib::wrapper! {
    /// Preferences page letting the user configure display mode, resolution, and framerate
    /// before starting a stream.
    pub struct DisplaySettingsView(ObjectSubclass<imp::DisplaySettingsView>)
        @extends gtk4::Widget, gtk4::Box,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl DisplaySettingsView {
    /// Instantiates a new `DisplaySettingsView`.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Builds and populates the preferences rows (mode, resolution, framerate) with their
    /// default selections.
    fn setup_ui(&self) {
        let imp = self.imp();
        self.set_orientation(gtk4::Orientation::Vertical);

        imp.preferences_group.set_title(i18n::tr("settings_title"));
        imp.preferences_group
            .set_description(Some(i18n::tr("settings_desc")));
        // Standard Adwaita preferences-page breathing room; a bare Box (unlike
        // AdwPreferencesPage) doesn't add this on its own.
        imp.preferences_group.set_margin_top(24);
        imp.preferences_group.set_margin_bottom(24);
        imp.preferences_group.set_margin_start(24);
        imp.preferences_group.set_margin_end(24);

        // 1. Display Mode selector (Extend vs Mirror)
        imp.mode_row.set_title(i18n::tr("mode_title"));
        imp.mode_row.set_subtitle(i18n::tr("mode_subtitle"));
        imp.mode_row
            .add_prefix(&gtk4::Image::from_icon_name("video-display-symbolic"));
        let modes = gtk4::StringList::new(&[i18n::tr("mode_extend"), i18n::tr("mode_mirror")]);
        imp.mode_row.set_model(Some(&modes));
        imp.mode_row.set_selected(0); // Default: Extend Desktop

        // 2. Resolution selector
        imp.resolution_row.set_title(i18n::tr("res_title"));
        imp.resolution_row.set_subtitle(i18n::tr("res_subtitle"));
        imp.resolution_row
            .add_prefix(&gtk4::Image::from_icon_name("display-symbolic"));
        let resolutions =
            gtk4::StringList::new(&[i18n::tr("res_native"), "1920x1200", "1920x1080"]);
        imp.resolution_row.set_model(Some(&resolutions));

        // 3. Framerate selector
        imp.framerate_row.set_title(i18n::tr("fps_title"));
        imp.framerate_row.set_subtitle(i18n::tr("fps_subtitle"));
        imp.framerate_row.add_prefix(&gtk4::Image::from_icon_name(
            "media-playback-start-symbolic",
        ));
        let framerates =
            gtk4::StringList::new(&[i18n::tr("fps_60"), i18n::tr("fps_120"), i18n::tr("fps_30")]);
        imp.framerate_row.set_model(Some(&framerates));

        imp.preferences_group.add(&imp.mode_row);
        imp.preferences_group.add(&imp.resolution_row);
        imp.preferences_group.add(&imp.framerate_row);

        self.append(&imp.preferences_group);
    }

    /// Reads the current UI selections and builds a `StreamConfig` reflecting them.
    pub fn get_stream_config(&self) -> StreamConfig {
        let imp = self.imp();

        let display_mode = match imp.mode_row.selected() {
            1 => DisplayMode::Mirror,
            _ => DisplayMode::Extend,
        };

        let (width, height) = match imp.resolution_row.selected() {
            1 => (1920, 1200),
            2 => (1920, 1080),
            _ => (2560, 1600),
        };

        let fps = match imp.framerate_row.selected() {
            1 => 120,
            2 => 30,
            _ => 60,
        };

        tracing::info!(
            "get_stream_config: mode_row.selected()={} -> {:?}, {}x{}@{}fps",
            imp.mode_row.selected(),
            display_mode,
            width,
            height,
            fps
        );

        StreamConfig {
            width,
            height,
            fps,
            encoder: "vaapi".to_string(),
            display_mode,
            device_serial: None,
            video_port: 6000,
            input_port: 6001,
        }
    }
}

impl Default for DisplaySettingsView {
    fn default() -> Self {
        Self::new()
    }
}
