use crate::daemon::adb_transport::AdbTransportManager;
use crate::daemon::{NibDaemon, StreamConfig};
use crate::i18n;
use crate::indicator::DeviceInfo;
use gtk4::glib;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

/// Shared Tokio multi-threaded runtime used to drive async daemon/portal calls from the
/// GTK main thread via `glib::MainContext::spawn_local`.
static TOKIO_RT: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime")
});

/// How many devices can hold a port pair at once. Bounds the port range Nib occupies to
/// 6000..=6019, matching what the client's `adb reverse` mapping expects.
const MAX_DEVICE_SLOTS: u16 = 10;

/// Maps a slot index to its (video, input) TCP port pair.
fn ports_for_slot(slot: u16) -> (u16, u16) {
    (6000 + slot * 2, 6001 + slot * 2)
}

/// Returns the slot `serial` already holds in `slots`, or claims the lowest free one for it.
/// `None` once all `MAX_DEVICE_SLOTS` slots are taken.
fn claim_slot(slots: &mut HashMap<String, u16>, serial: &str) -> Option<u16> {
    if let Some(slot) = slots.get(serial) {
        return Some(*slot);
    }
    let slot = (0..MAX_DEVICE_SLOTS).find(|candidate| !slots.values().any(|s| s == candidate))?;
    slots.insert(serial.to_string(), slot);
    Some(slot)
}

/// Closure supplied by the settings page to build a `StreamConfig` on demand.
type ConfigProvider = Rc<RefCell<Option<Box<dyn Fn() -> StreamConfig>>>>;

/// GObject subclass implementation details for `ConnectionStatusPage`.
mod imp {
    use super::*;

    /// Private instance state backing the `ConnectionStatusPage` GObject subclass.
    #[derive(Default)]
    pub struct ConnectionStatusPage {
        pub status_page: adw::StatusPage,
        pub toast_overlay: adw::ToastOverlay,
        /// Preferences group listing each connected device as an action row.
        pub devices_group: adw::PreferencesGroup,
        /// Action rows currently displayed, kept in sync with `known_devices`.
        pub device_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
        /// Active streaming daemons keyed by ADB device serial.
        pub daemons: Rc<RefCell<HashMap<String, NibDaemon>>>,
        pub config_provider: ConfigProvider,
        /// Serials of devices seen on the last device-monitoring poll.
        pub known_devices: Rc<RefCell<Vec<String>>>,
        /// Port slot reserved for each device serial, released when the device goes away with
        /// no daemon still attached to it. See `ConnectionStatusPage::ports_for_serial`.
        pub port_slots: Rc<RefCell<HashMap<String, u16>>>,
        /// Cache of resolved (display name, is_tablet) per serial, to avoid repeated ADB queries.
        pub cached_details: Rc<RefCell<HashMap<String, (String, bool)>>>,
        /// Last rendered device info list, used to skip UI rebuilds when nothing changed.
        pub cached_device_infos: Rc<RefCell<Vec<DeviceInfo>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ConnectionStatusPage {
        const NAME: &'static str = "ConnectionStatusPage";
        type Type = super::ConnectionStatusPage;
        type ParentType = gtk4::Box;
    }

    impl ObjectImpl for ConnectionStatusPage {
        /// Builds the status page UI and starts device polling once construction finishes.
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_ui();
        }
    }

    impl WidgetImpl for ConnectionStatusPage {}
    impl BoxImpl for ConnectionStatusPage {}
}

glib::wrapper! {
    /// Main connection page listing connected Android/iOS devices, gesture help, and
    /// controls for starting/stopping their streams.
    pub struct ConnectionStatusPage(ObjectSubclass<imp::ConnectionStatusPage>)
        @extends gtk4::Widget, gtk4::Box,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Orientable;
}

impl ConnectionStatusPage {
    /// Instantiates a new `ConnectionStatusPage`.
    pub fn new() -> Self {
        glib::Object::builder().build()
    }
}

impl Default for ConnectionStatusPage {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionStatusPage {
    /// Returns the (video, input) TCP port pair reserved for `serial`, claiming the lowest
    /// free slot the first time the device is seen and returning that same pair on every
    /// later call. Returns `None` once all `MAX_DEVICE_SLOTS` slots are taken.
    ///
    /// Deriving the slot from a hash of the serial instead would be simpler, but two
    /// simultaneously connected devices could then land on the same pair - ADB serials from
    /// one vendor share a prefix and differ in very few bytes - and the second stream's
    /// `TcpListener::bind` would fail after the session had already been set up. Handing out
    /// slots from a pool makes a collision impossible by construction.
    fn ports_for_serial(&self, serial: &str) -> Option<(u16, u16)> {
        let mut slots = self.imp().port_slots.borrow_mut();
        let had_slot = slots.contains_key(serial);
        match claim_slot(&mut slots, serial) {
            Some(slot) => {
                if !had_slot {
                    tracing::info!("Reserved port slot {} for device {}", slot, serial);
                }
                Some(ports_for_slot(slot))
            }
            None => {
                tracing::warn!(
                    "No free port slot left for device {} ({} slots in use)",
                    serial,
                    slots.len()
                );
                None
            }
        }
    }

    /// Releases the port slots of devices that are neither connected nor still owned by a
    /// daemon, so a long-running session doesn't exhaust the pool as devices come and go.
    /// A device that merely blinks out of `adb devices` while a stream is running keeps its
    /// slot, since its daemon still holds the bound ports.
    fn release_unused_port_slots(&self, connected: &[String]) {
        let imp = self.imp();
        let streaming: Vec<String> = imp.daemons.borrow().keys().cloned().collect();
        imp.port_slots.borrow_mut().retain(|serial, slot| {
            let keep = connected.contains(serial) || streaming.contains(serial);
            if !keep {
                tracing::info!("Released port slot {} held by device {}", slot, serial);
            }
            keep
        });
    }

    /// Registers the closure used to build a `StreamConfig` from the settings page when a
    /// new stream is started.
    pub fn set_config_provider<F: Fn() -> StreamConfig + 'static>(&self, provider: F) {
        *self.imp().config_provider.borrow_mut() = Some(Box::new(provider));
    }

    /// Stops every currently active device stream and clears the daemon map.
    pub fn stop_active_stream(&self) {
        let daemons_ref = self.imp().daemons.clone();
        glib::MainContext::default().spawn_local(async move {
            let _guard = TOKIO_RT.enter();
            let mut active_daemons = {
                let mut map = daemons_ref.borrow_mut();
                std::mem::take(&mut *map)
            };
            for (_, mut daemon) in active_daemons.drain() {
                if daemon.is_streaming {
                    daemon.stop_stream().await;
                }
            }
        });
    }

    /// Returns whether any device currently has an active stream.
    pub fn is_streaming(&self) -> bool {
        if let Ok(daemons) = self.imp().daemons.try_borrow() {
            return daemons.values().any(|d| d.is_streaming);
        }
        false
    }

    /// Toggles streaming for the first known connected device, if any.
    pub fn toggle_stream(&self) {
        let devices = self.imp().known_devices.borrow().clone();
        if let Some(first_serial) = devices.first() {
            self.toggle_stream_for_device(first_serial);
        }
    }

    /// Shows a message as an in-app toast if the window is focused, otherwise as a GNOME
    /// desktop notification.
    pub fn notify_user(&self, message: &str, notification_id: &str) {
        let is_window_active = self
            .native()
            .and_then(|n| n.downcast::<gtk4::Window>().ok())
            .map(|w| w.is_active())
            .unwrap_or(false);

        tracing::info!(
            "notify_user triggered: message='{}', id='{}', is_window_active={}",
            message,
            notification_id,
            is_window_active
        );

        if is_window_active {
            tracing::info!("Displaying in-app Libadwaita Toast overlay");
            self.imp().toast_overlay.add_toast(adw::Toast::new(message));
        } else if let Some(app) = gio::Application::default() {
            tracing::info!("Sending GNOME Desktop Notification via GIO Application");
            let notification = gio::Notification::new("Nib");
            notification.set_body(Some(message));
            notification.set_icon(&gio::ThemedIcon::new("dialog-warning-symbolic"));
            app.send_notification(Some(notification_id), &notification);
        }
    }

    /// Starts a stream for the given device serial, or stops it if one is already active.
    /// Reuses per-serial ports so multiple devices can stream concurrently.
    pub fn toggle_stream_for_device(&self, serial: &str) {
        // Reserved up front, on the GTK thread, so the slot is claimed before any other device
        // can be handled and so an exhausted pool is reported instead of failing later inside
        // the pipeline's `TcpListener::bind`.
        let Some((v_port, i_port)) = self.ports_for_serial(serial) else {
            self.notify_user(i18n::tr("no_free_ports"), "no-free-ports");
            return;
        };

        let serial_string = serial.to_string();
        let daemons_ref = self.imp().daemons.clone();
        let provider_ref = self.imp().config_provider.clone();
        let self_weak = self.downgrade();

        glib::MainContext::default().spawn_local(async move {
            let _guard = TOKIO_RT.enter();

            let mut existing_daemon = {
                let mut daemons = daemons_ref.borrow_mut();
                daemons.remove(&serial_string)
            };

            if let Some(mut daemon) = existing_daemon.take() {
                if daemon.is_streaming {
                    tracing::info!("Stopping stream for device {}", serial_string);
                    daemon.stop_stream().await;
                    return;
                }
            }

            tracing::info!("Starting stream for device {}", serial_string);
            let mut config = if let Some(provider) = &*provider_ref.borrow() {
                tracing::info!("Building StreamConfig from the settings page's config_provider");
                provider()
            } else {
                tracing::warn!(
                    "No config_provider registered; falling back to StreamConfig::default() (display_mode will be Extend regardless of any UI selection)"
                );
                StreamConfig::default()
            };

            config.device_serial = Some(serial_string.clone());
            config.video_port = v_port;
            config.input_port = i_port;

            let mut daemon = NibDaemon::new(config);
            match daemon.start_stream().await {
                Ok(()) => {
                    daemons_ref.borrow_mut().insert(serial_string, daemon);
                }
                Err(e) => {
                    if let Some(page) = self_weak.upgrade() {
                        if e.is_cancelled() {
                            tracing::info!(
                                "Stream request cancelled by user for device {}",
                                serial_string
                            );
                            page.notify_user(i18n::tr("stream_cancelled"), "stream-cancel");
                        } else {
                            tracing::error!("Failed to start stream for {}: {}", serial_string, e);
                            page.notify_user(
                                &format!("{}: {}", i18n::tr("stream_error"), e),
                                "stream-error",
                            );
                        }
                    }
                }
            }
        });
    }

    /// Builds the status page layout: device list, gesture help rows, and starts device polling.
    fn setup_ui(&self) {
        let imp = self.imp();
        self.set_orientation(gtk4::Orientation::Vertical);

        imp.status_page
            .set_icon_name(Some("video-display-symbolic"));
        imp.status_page.set_title(i18n::tr("app_title"));

        imp.devices_group.set_title(i18n::tr("devices_group_title"));

        let gestures_group = adw::PreferencesGroup::new();
        gestures_group.set_title(i18n::tr("gestures_group_title"));

        let row1 = adw::ActionRow::new();
        row1.set_title(i18n::tr("row1_title"));
        row1.set_subtitle(i18n::tr("row1_subtitle"));
        row1.add_prefix(&gtk4::Image::from_icon_name("input-touchpad-symbolic"));

        let row2 = adw::ActionRow::new();
        row2.set_title(i18n::tr("row2_title"));
        row2.set_subtitle(i18n::tr("row2_subtitle"));
        row2.add_prefix(&gtk4::Image::from_icon_name("input-mouse-symbolic"));

        let row3 = adw::ActionRow::new();
        row3.set_title(i18n::tr("row3_title"));
        row3.set_subtitle(i18n::tr("row3_subtitle"));
        row3.add_prefix(&gtk4::Image::from_icon_name("input-mouse-symbolic"));

        let row4 = adw::ActionRow::new();
        row4.set_title(i18n::tr("row4_title"));
        row4.set_subtitle(i18n::tr("row4_subtitle"));
        row4.add_prefix(&gtk4::Image::from_icon_name("view-grid-symbolic"));

        gestures_group.add(&row1);
        gestures_group.add(&row2);
        gestures_group.add(&row3);
        gestures_group.add(&row4);

        let button_box = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        button_box.append(&imp.devices_group);
        button_box.append(&gestures_group);

        imp.status_page.set_child(Some(&button_box));
        imp.toast_overlay.set_child(Some(&imp.status_page));
        self.append(&imp.toast_overlay);

        self.start_device_monitoring();
    }

    /// Polls `adb devices` every 3 seconds on the Tokio runtime and applies updates on the
    /// GTK main thread.
    fn start_device_monitoring(&self) {
        let self_weak = self.downgrade();
        glib::timeout_add_seconds_local(3, move || {
            let page = match self_weak.upgrade() {
                Some(p) => p,
                None => return glib::ControlFlow::Break,
            };

            let self_weak_async = page.downgrade();
            glib::MainContext::default().spawn_local(async move {
                let devices = TOKIO_RT
                    .spawn_blocking(AdbTransportManager::list_devices)
                    .await
                    .unwrap_or_default();

                if let Some(page) = self_weak_async.upgrade() {
                    page.handle_device_update(devices).await;
                }
            });

            glib::ControlFlow::Continue
        });
    }

    /// Reconciles the latest device list against known state: stops streams for unplugged
    /// devices, resolves display names/form factor for new devices, and rebuilds the device
    /// rows and tray indicator only if the device set actually changed.
    async fn handle_device_update(&self, mut devices: Vec<String>) {
        let imp = self.imp();
        devices.sort();

        // Detect streams whose portal session was closed externally - most notably GNOME
        // Shell's own screen-sharing system indicator ("Turn Off" button) - rather than
        // through nib's "Stop stream" control. Without this, `is_streaming` stays stuck true
        // and the device keeps showing as connected even though the phone/tablet already
        // disconnected on its end.
        let externally_stopped_serials: Vec<String> = {
            let daemons = imp.daemons.borrow();
            daemons
                .iter()
                .filter(|(_, d)| d.is_streaming && !d.is_session_alive())
                .map(|(serial, _)| serial.clone())
                .collect()
        };

        if !externally_stopped_serials.is_empty() {
            let daemons_ref = imp.daemons.clone();
            let self_weak = self.downgrade();

            glib::MainContext::default().spawn_local(async move {
                let _guard = TOKIO_RT.enter();
                for serial in externally_stopped_serials {
                    let daemon_opt = {
                        let mut map = daemons_ref.borrow_mut();
                        map.remove(&serial)
                    };
                    if let Some(mut daemon) = daemon_opt {
                        tracing::info!(
                            "Stream for device {} was stopped externally (system screen-sharing control), tearing down",
                            serial
                        );
                        daemon.stop_stream().await;
                        if let Some(page) = self_weak.upgrade() {
                            page.notify_user(
                                &format!("{} ({})", i18n::tr("stream_stopped_externally"), serial),
                                &format!("external-stop-{}", serial),
                            );
                        }
                    }
                }
            });
        }

        // Detect devices that were unplugged and stop their active stream daemons
        let unplugged_serials: Vec<String> = {
            let daemons = imp.daemons.borrow();
            daemons
                .keys()
                .filter(|s| !devices.contains(s))
                .cloned()
                .collect()
        };

        if !unplugged_serials.is_empty() {
            let daemons_ref = imp.daemons.clone();
            let self_weak = self.downgrade();
            let unplugged_clone = unplugged_serials.clone();

            glib::MainContext::default().spawn_local(async move {
                let _guard = TOKIO_RT.enter();
                for serial in unplugged_clone {
                    let mut daemon_opt = {
                        let mut map = daemons_ref.borrow_mut();
                        map.remove(&serial)
                    };
                    if let Some(mut daemon) = daemon_opt.take() {
                        tracing::info!("Device {} unplugged, stopping stream daemon", serial);
                        daemon.stop_stream().await;
                        if let Some(page) = self_weak.upgrade() {
                            page.notify_user(
                                &format!("{} ({})", i18n::tr("device_disconnected"), serial),
                                &format!("unplug-{}", serial),
                            );
                        }
                    }
                }
            });
        }

        *imp.known_devices.borrow_mut() = devices.clone();
        self.release_unused_port_slots(&devices);

        let mut device_infos = Vec::new();
        for serial in &devices {
            let cached_opt = imp.cached_details.borrow().get(serial).cloned();
            let (name, is_tablet) = if let Some(cached) = cached_opt {
                cached
            } else {
                let s_clone = serial.clone();
                let fetched = TOKIO_RT
                    .spawn_blocking(move || AdbTransportManager::get_device_details(Some(&s_clone)))
                    .await
                    .unwrap_or_else(|_| (format!("Android Device ({})", serial), true));

                imp.cached_details
                    .borrow_mut()
                    .insert(serial.clone(), fetched.clone());
                fetched
            };

            let is_streaming = imp
                .daemons
                .borrow()
                .get(serial)
                .map(|d| d.is_streaming)
                .unwrap_or(false);

            device_infos.push(DeviceInfo {
                serial: serial.clone(),
                name,
                is_tablet,
                is_streaming,
            });
        }

        if *imp.cached_device_infos.borrow() != device_infos {
            *imp.cached_device_infos.borrow_mut() = device_infos.clone();

            // Remove previous Libadwaita ActionRows
            for row in imp.device_rows.borrow_mut().drain(..) {
                imp.devices_group.remove(&row);
            }

            if device_infos.is_empty() {
                let row = adw::ActionRow::new();
                row.set_title(i18n::tr("no_devices_title"));
                row.set_subtitle(i18n::tr("no_devices_subtitle"));
                row.add_prefix(&gtk4::Image::from_icon_name("display-symbolic"));
                imp.devices_group.add(&row);
                imp.device_rows.borrow_mut().push(row);
            } else {
                for info in &device_infos {
                    let row = adw::ActionRow::new();
                    let icon_name = if info.is_tablet {
                        "tablet-symbolic"
                    } else {
                        "phone-symbolic"
                    };
                    row.set_title(&info.name);
                    row.set_subtitle(&match self.ports_for_serial(&info.serial) {
                        Some((v_port, i_port)) => format!(
                            "ADB Serial: {} • Ports: Video {} / Input {}",
                            info.serial, v_port, i_port
                        ),
                        None => format!(
                            "ADB Serial: {} • {}",
                            info.serial,
                            i18n::tr("no_free_ports")
                        ),
                    });
                    row.add_prefix(&gtk4::Image::from_icon_name(icon_name));

                    let btn = gtk4::Button::new();
                    if info.is_streaming {
                        btn.set_label(i18n::tr("stop_stream"));
                        btn.add_css_class("destructive-action");
                    } else {
                        btn.set_label(i18n::tr("start_stream"));
                        btn.add_css_class("suggested-action");
                    }
                    btn.add_css_class("pill");
                    btn.set_valign(gtk4::Align::Center);

                    let self_clone = self.clone();
                    let serial_clone = info.serial.clone();
                    btn.connect_clicked(move |_| {
                        self_clone.toggle_stream_for_device(&serial_clone);
                    });

                    row.add_suffix(&btn);
                    imp.devices_group.add(&row);
                    imp.device_rows.borrow_mut().push(row);
                }
            }

            // Update Top Bar Indicator
            if let Some(app) = gio::Application::default() {
                if let Ok(nib_app) = app.downcast::<crate::application::NibApplication>() {
                    if let Some(indicator) = nib_app.indicator() {
                        indicator.update_devices(device_infos);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_maps_to_its_port_pair() {
        assert_eq!(ports_for_slot(0), (6000, 6001));
        assert_eq!(ports_for_slot(1), (6002, 6003));
        assert_eq!(ports_for_slot(MAX_DEVICE_SLOTS - 1), (6018, 6019));
    }

    #[test]
    fn same_serial_keeps_its_slot() {
        let mut slots = HashMap::new();
        let first = claim_slot(&mut slots, "ABC123").unwrap();
        assert_eq!(claim_slot(&mut slots, "ABC123"), Some(first));
        assert_eq!(slots.len(), 1);
    }

    #[test]
    fn distinct_serials_never_share_a_slot() {
        // Two pairs of realistic ADB serials that each collide under the sum-of-bytes hash the
        // old scheme derived slots from: transposing any two characters leaves the sum, and so
        // the slot, unchanged. Both pairs used to be handed the same port pair.
        let serials = ["R58N12ABCDE", "R58N12ABCED", "ZY22GHKLMN", "ZY22GHKLNM"];
        let mut slots = HashMap::new();
        let mut assigned = Vec::new();
        for serial in serials {
            assigned.push(claim_slot(&mut slots, serial).unwrap());
        }
        let mut unique = assigned.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), assigned.len(), "slots handed out twice");
    }

    #[test]
    fn claims_the_lowest_free_slot() {
        let mut slots = HashMap::new();
        for i in 0..3 {
            claim_slot(&mut slots, &format!("device{}", i));
        }
        slots.remove("device1");
        assert_eq!(claim_slot(&mut slots, "newcomer"), Some(1));
    }

    #[test]
    fn pool_exhaustion_reports_none_instead_of_reusing() {
        let mut slots = HashMap::new();
        for i in 0..MAX_DEVICE_SLOTS {
            assert!(claim_slot(&mut slots, &format!("device{}", i)).is_some());
        }
        assert_eq!(claim_slot(&mut slots, "one-too-many"), None);
        assert_eq!(slots.len(), MAX_DEVICE_SLOTS as usize);
    }
}
