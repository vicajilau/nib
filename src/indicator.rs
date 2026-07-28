use crate::i18n;
use ksni::blocking::Handle;
use ksni::blocking::TrayMethods;
use ksni::menu::*;
use std::sync::Arc;

/// Metadata representing a connected mobile device and its active video streaming state.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub serial: String,
    pub name: String,
    pub is_tablet: bool,
    pub is_streaming: bool,
}

/// Freedesktop StatusNotifierItem (KSNI) system tray indicator.
pub struct NibTray {
    pub devices: Vec<DeviceInfo>,
    pub show_window_cb: Arc<dyn Fn() + Send + Sync>,
    pub toggle_device_cb: Arc<dyn Fn(String) + Send + Sync>,
    pub quit_cb: Arc<dyn Fn() + Send + Sync>,
}

impl ksni::Tray for NibTray {
    /// Freedesktop StatusNotifierItem unique identifier for this tray icon.
    fn id(&self) -> String {
        "dev.victorcarreras.Nib".into()
    }

    /// Display title shown for the tray icon.
    fn title(&self) -> String {
        "Nib".into()
    }

    /// Symbolic icon name rendered in the top bar.
    fn icon_name(&self) -> String {
        "video-display-symbolic".into()
    }

    /// Reports `Active` when any device is connected or streaming, `Passive` otherwise.
    fn status(&self) -> ksni::Status {
        let any_active = self.devices.iter().any(|d| d.is_streaming) || !self.devices.is_empty();
        if any_active {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    /// Builds the hover tooltip summarizing connected and streaming devices.
    fn tool_tip(&self) -> ksni::ToolTip {
        let streaming_devices: Vec<_> = self.devices.iter().filter(|d| d.is_streaming).collect();

        let description = if !streaming_devices.is_empty() {
            let names: Vec<_> = streaming_devices.iter().map(|d| d.name.as_str()).collect();
            format!("{} {}", i18n::tr("tray_streaming_active"), names.join(", "))
        } else if !self.devices.is_empty() {
            let names: Vec<_> = self.devices.iter().map(|d| d.name.as_str()).collect();
            format!(
                "{} {}",
                i18n::tr("tray_devices_connected"),
                names.join(", ")
            )
        } else {
            i18n::tr("tray_no_devices").into()
        };

        ksni::ToolTip {
            title: "Nib".into(),
            description,
            icon_name: self.icon_name(),
            icon_pixmap: Vec::new(),
        }
    }

    /// Builds the tray context menu: one start/stop entry per device, plus show-window and quit.
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items = Vec::new();

        if self.devices.is_empty() {
            items.push(
                StandardItem {
                    label: i18n::tr("tray_no_devices").into(),
                    enabled: false,
                    icon_name: "display-symbolic".into(),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            for dev in &self.devices {
                let action_text = if dev.is_streaming {
                    i18n::tr("stop_stream")
                } else {
                    i18n::tr("start_stream")
                };
                let stream_label = format!("{} ({})", action_text, dev.name);
                let item_icon = if dev.is_tablet {
                    "tablet-symbolic"
                } else {
                    "phone-symbolic"
                };

                let serial_clone = dev.serial.clone();
                let toggle_cb = self.toggle_device_cb.clone();

                let item = StandardItem {
                    label: stream_label,
                    enabled: true,
                    icon_name: item_icon.into(),
                    activate: Box::new(move |_| {
                        let serial = serial_clone.clone();
                        let cb = toggle_cb.clone();
                        cb(serial);
                    }),
                    ..Default::default()
                };

                items.push(MenuItem::Standard(item));
            }
        }

        let show_cb = self.show_window_cb.clone();
        let show_item = StandardItem {
            label: i18n::tr("tray_show").into(),
            enabled: true,
            icon_name: "window-pop-out-symbolic".into(),
            activate: Box::new(move |_| {
                show_cb();
            }),
            ..Default::default()
        };

        let q_cb = self.quit_cb.clone();
        let quit_item = StandardItem {
            label: i18n::tr("tray_quit").into(),
            enabled: true,
            icon_name: "application-exit-symbolic".into(),
            activate: Box::new(move |_| {
                q_cb();
            }),
            ..Default::default()
        };

        items.push(MenuItem::Separator);
        items.push(MenuItem::Standard(show_item));
        items.push(MenuItem::Separator);
        items.push(MenuItem::Standard(quit_item));

        items
    }
}

/// Thread-safe manager handle for updating top bar tray menus and device status notifications.
pub struct IndicatorManager {
    /// Handle to the background-spawned KSNI tray, used to push device list updates.
    handle: Handle<NibTray>,
}

impl IndicatorManager {
    /// Creates and spawns a new background system tray indicator instance.
    pub fn new(
        show_window_cb: Arc<dyn Fn() + Send + Sync>,
        toggle_device_cb: Arc<dyn Fn(String) + Send + Sync>,
        quit_cb: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let tray = NibTray {
            devices: Vec::new(),
            show_window_cb,
            toggle_device_cb,
            quit_cb,
        };

        let handle = tray
            .spawn()
            .unwrap_or_else(|e| panic!("Failed to spawn top bar indicator tray: {}", e));

        Self { handle }
    }

    /// Dynamically updates the active list of connected devices in the system tray context menu.
    pub fn update_devices(&self, devices: Vec<DeviceInfo>) {
        self.handle.update(move |tray| {
            tray.devices = devices;
        });
    }
}
