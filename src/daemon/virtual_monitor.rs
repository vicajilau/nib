use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use zbus::zvariant::OwnedValue;

/// Selects how the streamed display is exposed on the GNOME desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// Creates a new independent Virtual Display in GNOME (Extended Desktop).
    Extend,
    /// Captures an existing physical monitor (Screen Mirroring).
    Mirror,
}

/// Mutter `DisplayConfig.GetCurrentState` D-Bus monitor entry:
/// `(connector, vendor, product, serial)`, its supported modes, and monitor properties.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
pub struct DisplayMonitor(
    (String, String, String, String),
    Vec<DisplayModeSpec>,
    std::collections::HashMap<String, OwnedValue>,
);

/// Mutter `DisplayConfig` mode descriptor: id, width, height, refresh rate, scale,
/// supported scales, and mode properties.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
pub struct DisplayModeSpec(
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    std::collections::HashMap<String, OwnedValue>,
);

/// Mutter `DisplayConfig` logical monitor entry: `(x, y, scale, transform, is_primary,
/// monitors, properties)` describing a monitor's position within the global desktop layout.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize, zbus::zvariant::Type)]
pub struct LogicalMonitor(
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<(String, String, String, String)>,
    std::collections::HashMap<String, OwnedValue>,
);

/// Manages the Freedesktop RemoteDesktop/ScreenCast portal session used to create the virtual
/// or mirrored display, inject input events, and track its logical position on the desktop.
pub struct VirtualMonitorManager {
    node_id: Option<u32>,
    pipewire_fd: Option<OwnedFd>,
    remote_desktop: Option<RemoteDesktop>,
    session: Option<Session<RemoteDesktop>>,
    /// Whether the display session extends the desktop or mirrors an existing monitor.
    pub display_mode: DisplayMode,
    width: u32,
    height: u32,
    /// Tracks whether the most recent pointer input targeted the virtual/mirrored display,
    /// so the cursor can be reset to the primary screen on teardown.
    pub was_last_input_on_virtual: AtomicBool,

    x_offset: AtomicI32,
    y_offset: AtomicI32,
    actual_width: AtomicI32,
    actual_height: AtomicI32,
    last_abs_x: Mutex<f64>,
    last_abs_y: Mutex<f64>,
}

impl VirtualMonitorManager {
    /// Creates a new, unconnected `VirtualMonitorManager` with default mirror-mode dimensions.
    pub async fn new() -> Result<Self, String> {
        Ok(Self {
            node_id: None,
            pipewire_fd: None,
            remote_desktop: None,
            session: None,
            display_mode: DisplayMode::Mirror,
            width: 2560,
            height: 1600,
            was_last_input_on_virtual: AtomicBool::new(false),
            x_offset: AtomicI32::new(0),
            y_offset: AtomicI32::new(0),
            actual_width: AtomicI32::new(2560),
            actual_height: AtomicI32::new(1600),
            last_abs_x: Mutex::new(0.0),
            last_abs_y: Mutex::new(0.0),
        })
    }

    /// Updates the target/actual display resolution from a client handshake, ignoring
    /// zero-valued dimensions.
    pub fn update_target_resolution(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            tracing::info!(
                "Auto-negotiated tablet screen resolution: {}x{} (Aspect Ratio: {:.2}:1)",
                width,
                height,
                width as f64 / height as f64
            );
            self.width = width;
            self.height = height;
            self.actual_width.store(width as i32, Ordering::SeqCst);
            self.actual_height.store(height as i32, Ordering::SeqCst);
        }
    }

    /// Requests GNOME Mutter / Portal to capture ScreenCast display with explicit PipeWire Remote FD and Input Injection
    pub async fn create_display(
        &mut self,
        mode: DisplayMode,
        width: u32,
        height: u32,
    ) -> Result<(Option<u32>, Option<i32>), String> {
        self.display_mode = mode;
        self.width = if width > 0 { width } else { 2560 };
        self.height = if height > 0 { height } else { 1600 };
        self.actual_width.store(self.width as i32, Ordering::SeqCst);
        self.actual_height
            .store(self.height as i32, Ordering::SeqCst);
        tracing::info!(
            "Connecting via Freedesktop RemoteDesktop & ScreenCast Portal (Mode: {:?}, {}x{})...",
            self.display_mode,
            self.width,
            self.height
        );

        let rd_proxy = RemoteDesktop::new().await.map_err(|e| e.to_string())?;
        let session = rd_proxy
            .create_session(Default::default())
            .await
            .map_err(|e| e.to_string())?;

        // 1. Select Input Devices (Pointer, Touchscreen & Keyboard)
        rd_proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default().set_devices(
                    DeviceType::Pointer | DeviceType::Touchscreen | DeviceType::Keyboard,
                ),
            )
            .await
            .map_err(|e| e.to_string())?;

        // 2. Select Screencast Sources
        let sc_proxy = Screencast::new().await.map_err(|e| e.to_string())?;
        sc_proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::Embedded)
                    .set_sources(SourceType::Monitor | SourceType::Virtual)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await
            .map_err(|e| e.to_string())?;

        // 3. Start combined RemoteDesktop session
        let response = rd_proxy
            .start(&session, None, Default::default())
            .await
            .map_err(|e| e.to_string())?
            .response()
            .map_err(|e| e.to_string())?;

        let fd = sc_proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .map_err(|e| format!("Failed to open PipeWire remote fd: {}", e))?;

        let raw_fd = fd.as_raw_fd();
        self.pipewire_fd = Some(fd);

        let node_id = if let Some(stream) = response.streams().first() {
            let id = stream.pipe_wire_node_id();
            if let Some((w, h)) = stream.size() {
                if w >= 640 && h >= 480 {
                    self.width = w as u32;
                    self.height = h as u32;
                    self.actual_width.store(w, Ordering::SeqCst);
                    self.actual_height.store(h, Ordering::SeqCst);
                    tracing::info!("GNOME Portal reported stream resolution: {}x{}", w, h);
                } else {
                    tracing::info!(
                        "GNOME Portal reported initial placeholder resolution ({}x{}); retaining target resolution ({}x{})",
                        w, h, self.width, self.height
                    );
                }
            }
            tracing::info!(
                "SUCCESS: ScreenCast PipeWire Node ID: {} with PipeWire Remote FD: {} (Res: {}x{})",
                id,
                raw_fd,
                self.width,
                self.height
            );
            Some(id)
        } else {
            None
        };

        self.node_id = node_id;
        self.remote_desktop = Some(rd_proxy);
        self.session = Some(session);

        if self.display_mode == DisplayMode::Extend {
            let _ = self.align_virtual_monitor().await;
        }

        Ok((node_id, Some(raw_fd)))
    }

    /// Injects absolute pointer motion coordinates onto the virtual display or mirror display.
    pub async fn notify_pointer_motion_absolute(
        &self,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);

        if self.display_mode == DisplayMode::Mirror {
            self.notify_motion_mirror(norm_x, norm_y).await
        } else {
            self.notify_motion_extend(norm_x, norm_y).await
        }
    }

    /// Internal absolute motion handler for screen mirroring mode.
    async fn notify_motion_mirror(&self, norm_x: f64, norm_y: f64) -> Result<(), String> {
        let w = self.actual_width.load(Ordering::SeqCst) as f64;
        let h = self.actual_height.load(Ordering::SeqCst) as f64;
        let px = (norm_x * w).clamp(0.0, w);
        let py = (norm_y * h).clamp(0.0, h);

        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            let stream_node_id = self.node_id.unwrap_or(0);
            tracing::info!(
                "Injecting Mirror Motion Stream Node ID {}: Pixel ({:.1}, {:.1}) (Display: {}x{})",
                stream_node_id,
                px,
                py,
                w,
                h
            );
            if let Err(e) = rd
                .notify_pointer_motion_absolute(session, stream_node_id, px, py, Default::default())
                .await
            {
                tracing::error!(
                    "notify_pointer_motion_absolute Error on Node ID {}: {}",
                    stream_node_id,
                    e
                );
            }
        } else {
            tracing::warn!("notify_pointer_motion_absolute skipped: rd or session missing!");
        }
        Ok(())
    }

    /// Internal absolute motion handler for extended desktop display mode.
    async fn notify_motion_extend(&self, norm_x: f64, norm_y: f64) -> Result<(), String> {
        let w = self.actual_width.load(Ordering::SeqCst) as f64;
        let h = self.actual_height.load(Ordering::SeqCst) as f64;
        let px = (norm_x * w).clamp(0.0, w);
        let py = (norm_y * h).clamp(0.0, h);

        let x_off = self.x_offset.load(Ordering::SeqCst) as f64;
        let y_off = self.y_offset.load(Ordering::SeqCst) as f64;

        let abs_x = x_off + px;
        let abs_y = y_off + py;

        let (dx, dy) = {
            let mut lx = self.last_abs_x.lock().unwrap();
            let mut ly = self.last_abs_y.lock().unwrap();
            let dx = if *lx == 0.0 { 0.0 } else { abs_x - *lx };
            let dy = if *ly == 0.0 { 0.0 } else { abs_y - *ly };
            *lx = abs_x;
            *ly = abs_y;
            (dx, dy)
        };

        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            let node_id = self.node_id.unwrap_or(0);
            tracing::info!(
                "EXTEND MOTION: Stream {} | Local ({:.1}, {:.1}) | Global ({:.1}, {:.1})",
                node_id,
                px,
                py,
                abs_x,
                abs_y
            );

            if rd
                .notify_pointer_motion_absolute(session, node_id, px, py, Default::default())
                .await
                .is_ok()
            {
                return Ok(());
            }
            if rd
                .notify_pointer_motion_absolute(session, node_id, abs_x, abs_y, Default::default())
                .await
                .is_ok()
            {
                return Ok(());
            }
            if rd
                .notify_pointer_motion_absolute(session, 0, abs_x, abs_y, Default::default())
                .await
                .is_ok()
            {
                return Ok(());
            }

            // Fallback: Relative Motion for Extend Mode
            let _ = rd
                .notify_pointer_motion(session, dx, dy, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects a mouse pointer button event (pressed or released) via the RemoteDesktop portal.
    pub async fn notify_pointer_button(&self, button: i32, state: KeyState) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!("Injecting Pointer Button: {} {:?}", button, state);
            rd.notify_pointer_button(session, button, state, Default::default())
                .await
                .map_err(|e| {
                    tracing::error!("Portal notify_pointer_button error: {}", e);
                    e.to_string()
                })?;
        } else {
            tracing::warn!("notify_pointer_button skipped: rd or session missing!");
        }
        Ok(())
    }

    /// Injects a contextual right-click event at normalized screen coordinates.
    pub async fn notify_right_click(&self, norm_x: f64, norm_y: f64) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!(
                "Injecting Right Click (BTN_RIGHT 273) at ({:.4}, {:.4})...",
                norm_x,
                norm_y
            );
            let _ = rd
                .notify_pointer_button(session, 272, KeyState::Released, Default::default())
                .await;
            let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
            let _ = rd
                .notify_pointer_button(session, 273, KeyState::Pressed, Default::default())
                .await;
            let _ = rd
                .notify_pointer_button(session, 273, KeyState::Released, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects a GNOME Overview toggle command using Super key keycode simulation.
    pub async fn notify_overview_toggle(&self) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!("Injecting GNOME Overview Gesture (Super Key 125)...");
            let _ = rd
                .notify_pointer_button(session, 272, KeyState::Released, Default::default())
                .await;
            let _ = rd
                .notify_keyboard_keycode(session, 125, KeyState::Pressed, Default::default())
                .await;
            let _ = rd
                .notify_keyboard_keycode(session, 125, KeyState::Released, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects scroll axis motion deltas for 2-axis page scrolling.
    pub async fn notify_scroll(&self, dx: f64, dy: f64) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            let _ = rd
                .notify_pointer_axis(session, dx, dy, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects a primary touch contact event at normalized screen coordinates.
    pub async fn notify_touch_down(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);

        tracing::info!(
            "TOUCH DOWN slot {} -> Pointer Motion ({:.4}, {:.4}) + BTN_LEFT Pressed",
            slot,
            norm_x,
            norm_y
        );
        let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        let _ = self.notify_pointer_button(272, KeyState::Pressed).await;
        Ok(())
    }

    /// Injects touch motion movement at normalized screen coordinates.
    pub async fn notify_touch_motion(
        &self,
        _slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);

        let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        Ok(())
    }

    /// Injects touch release event releasing primary pointer button.
    pub async fn notify_touch_up(&self, slot: u32) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!("TOUCH UP slot {} -> BTN_LEFT Released", slot);
            let _ = rd
                .notify_pointer_button(session, 272, KeyState::Released, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects active stylus down contact event with pressure and tilt parameters.
    pub async fn notify_stylus_down(
        &self,
        norm_x: f64,
        norm_y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    ) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);
        tracing::info!(
            "STYLUS DOWN -> Norm ({:.4}, {:.4}) Pressure: {:.2} Tilt: ({:.1}, {:.1})",
            norm_x,
            norm_y,
            pressure,
            tilt_x,
            tilt_y
        );
        let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        let _ = self.notify_pointer_button(272, KeyState::Pressed).await;
        Ok(())
    }

    /// Injects active stylus motion event with pressure and tilt parameters.
    pub async fn notify_stylus_move(
        &self,
        norm_x: f64,
        norm_y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    ) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);
        tracing::trace!(
            "STYLUS MOVE -> Norm ({:.4}, {:.4}) Pressure: {:.2} Tilt: ({:.1}, {:.1})",
            norm_x,
            norm_y,
            pressure,
            tilt_x,
            tilt_y
        );
        let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        Ok(())
    }

    /// Injects active stylus lift event at normalized screen coordinates.
    pub async fn notify_stylus_up(&self, norm_x: f64, norm_y: f64) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);
        tracing::info!("STYLUS UP -> Norm ({:.4}, {:.4})", norm_x, norm_y);
        let _ = self.notify_pointer_button(272, KeyState::Released).await;
        let _ = self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        Ok(())
    }

    /// Handles continuous 1:1 swipe gestures and triggers overview toggle when threshold is met.
    pub async fn notify_swipe_gesture(&self, _dx: f64, dy: f64) -> Result<(), String> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        tracing::info!("SWIPE GESTURE -> dy: {:.2}", dy);
        if dy.abs() > 30.0 {
            let _ = self.notify_overview_toggle().await;
        }
        Ok(())
    }

    /// Queries Mutter's `DisplayConfig` D-Bus interface to locate the created virtual monitor
    /// (in `Extend` mode) and cache its logical offset and resolution for coordinate mapping.
    pub async fn align_virtual_monitor(&self) -> Result<(), String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| format!("Failed to connect to session bus: {}", e))?;

        let reply = connection
            .call_method(
                Some("org.gnome.Mutter.DisplayConfig"),
                "/org/gnome/Mutter/DisplayConfig",
                Some("org.gnome.Mutter.DisplayConfig"),
                "GetCurrentState",
                &(),
            )
            .await
            .map_err(|e| format!("Failed to call GetCurrentState: {}", e))?;

        let body = reply.body();
        let (serial, monitors, logical_monitors, _props): (
            u32,
            Vec<DisplayMonitor>,
            Vec<LogicalMonitor>,
            std::collections::HashMap<String, OwnedValue>,
        ) = match body.deserialize() {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Failed to deserialize GetCurrentState D-Bus reply: {}", e);
                return Err(format!("Failed to deserialize state: {}", e));
            }
        };

        tracing::info!(
            "D-Bus DisplayConfig state: Serial {}, logical monitors count: {}",
            serial,
            logical_monitors.len()
        );

        let mut found_virtual = false;
        if self.display_mode == DisplayMode::Extend {
            for lm in &logical_monitors {
                let x = lm.0;
                let y = lm.1;
                for spec in &lm.5 {
                    let connector = &spec.0;
                    let vendor = &spec.1;
                    if connector.starts_with("Meta")
                        || vendor.contains("Meta")
                        || connector.contains("Virtual")
                    {
                        self.x_offset.store(x, Ordering::SeqCst);
                        self.y_offset.store(y, Ordering::SeqCst);

                        for mon in &monitors {
                            let mon_spec = &mon.0;
                            if mon_spec.0 == *connector {
                                if let Some(mode) = mon.1.first() {
                                    self.actual_width.store(mode.1, Ordering::SeqCst);
                                    self.actual_height.store(mode.2, Ordering::SeqCst);
                                    tracing::info!(
                                        "SUCCESS: Found Virtual Display '{}' at Logical Offset ({}, {}), Resolution: {}x{}",
                                        connector, x, y, mode.1, mode.2
                                    );
                                }
                            }
                        }
                        found_virtual = true;
                        break;
                    }
                }
                if found_virtual {
                    break;
                }
            }
        }

        if !found_virtual {
            self.x_offset.store(0, Ordering::SeqCst);
            self.y_offset.store(0, Ordering::SeqCst);
            tracing::info!("Using standard display offset (0, 0) (Mirror Mode or Primary Display)");
        }

        Ok(())
    }

    /// Tears down the portal session, resetting the cursor to the primary display origin
    /// if the virtual/mirrored display was last touched.
    pub async fn destroy_display(&mut self) -> Result<(), String> {
        tracing::info!("Destroying Display Session");
        if self.was_last_input_on_virtual.swap(false, Ordering::SeqCst) {
            tracing::info!(
                "Last input was on virtual display; resetting cursor to primary screen origin."
            );
            let _ = self.notify_pointer_motion_absolute(0.0, 0.0).await;
        }
        if let Some(session) = self.session.take() {
            let _ = session.close().await;
        }
        self.node_id = None;
        self.pipewire_fd = None;
        self.remote_desktop = None;
        self.x_offset.store(0, Ordering::SeqCst);
        self.y_offset.store(0, Ordering::SeqCst);
        Ok(())
    }
}
