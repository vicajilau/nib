use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::ui::NibView;

/// GObject subclass implementation details for `NibWindow`.
mod imp {
    use super::*;

    /// Private instance state backing the `NibWindow` GObject subclass.
    #[derive(Default)]
    pub struct NibWindow {
        /// Root content view holding the connection status page and settings page.
        pub view: NibView,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NibWindow {
        const NAME: &'static str = "NibWindow";
        type Type = super::NibWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for NibWindow {
        /// Builds the window UI once the GObject instance has finished construction.
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_ui();
        }
    }

    impl WidgetImpl for NibWindow {}
    impl WindowImpl for NibWindow {}
    impl ApplicationWindowImpl for NibWindow {}
    impl AdwApplicationWindowImpl for NibWindow {}
}

glib::wrapper! {
    /// Main Libadwaita window managing window layout, header bar actions, and close-to-tray behaviors.
    pub struct NibWindow(ObjectSubclass<imp::NibWindow>)
        @extends gtk4::Widget, gtk4::Window, gtk4::ApplicationWindow, adw::ApplicationWindow,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager, gio::ActionGroup, gio::ActionMap;
}

impl NibWindow {
    /// Instantiates a new `NibWindow` attached to the GTK application instance.
    pub fn new(app: &crate::application::NibApplication) -> Self {
        glib::Object::builder()
            .property("application", app)
            .property("title", "Nib")
            .property("default-width", 800)
            .property("default-height", 600)
            .build()
    }

    /// Initializes header bar widgets, primary menu actions, and close-to-tray handlers.
    fn setup_ui(&self) {
        let imp = self.imp();

        let header_bar = adw::HeaderBar::new();

        // Without this, the "Display Settings" page (mode/resolution/framerate/stylus) built in
        // `NibView` is reachable in code but has no on-screen control to navigate to it.
        let view_switcher = adw::ViewSwitcher::new();
        view_switcher.set_stack(Some(&imp.view.view_stack()));
        view_switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
        header_bar.set_title_widget(Some(&view_switcher));

        let menu_button = gtk4::MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");
        menu_button.set_tooltip_text(Some("Main Menu"));

        let menu = gio::Menu::new();
        menu.append(Some("About Nib"), Some("app.about"));
        menu.append(Some("Quit"), Some("app.quit"));
        menu_button.set_menu_model(Some(&menu));

        header_bar.pack_end(&menu_button);

        let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        main_box.append(&header_bar);
        main_box.append(&imp.view);

        adw::prelude::AdwApplicationWindowExt::set_content(self, Some(&main_box));

        self.connect_close_request(move |win| {
            tracing::info!("Hiding window to top bar indicator...");
            win.set_visible(false);
            glib::Propagation::Stop
        });
    }

    /// Toggles active display streaming for the first available connected device.
    pub fn toggle_stream(&self) {
        self.imp().view.toggle_stream();
    }

    /// Toggles active display streaming for a specific device identified by serial number.
    pub fn toggle_stream_for_device(&self, serial: &str) {
        self.imp().view.toggle_stream_for_device(serial);
    }
}
