use crate::daemon::DaemonError;
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
    pub async fn new() -> Result<Self, DaemonError> {
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

    /// Requests GNOME Mutter / Portal to capture ScreenCast display with explicit PipeWire Remote FD and Input Injection.
    /// Returns the resolved node ID, PipeWire FD, and the `DisplayMode` actually granted by the
    /// compositor (see the note on `SourceType` reconciliation below), which may differ from the
    /// `mode` requested.
    pub async fn create_display(
        &mut self,
        mode: DisplayMode,
        width: u32,
        height: u32,
    ) -> Result<(Option<u32>, Option<i32>, DisplayMode), DaemonError> {
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

        let rd_proxy = RemoteDesktop::new().await?;
        let session = rd_proxy.create_session(Default::default()).await?;

        // 1. Select Input Devices (Pointer, Touchscreen & Keyboard)
        rd_proxy
            .select_devices(
                &session,
                SelectDevicesOptions::default().set_devices(
                    DeviceType::Pointer | DeviceType::Touchscreen | DeviceType::Keyboard,
                ),
            )
            .await?;

        // 2. Select Screencast Sources
        // Restricted to the type matching nib's own Mirror/Extend selection: the native picker
        // must only ever offer what the user asked nib for - physical screens for Mirror, the
        // virtual-monitor option (created without a picker at all) for Extend. Offering both
        // together would let the native dialog silently override the app's own setting.
        let requested_source: ashpd::enumflags2::BitFlags<SourceType> = match self.display_mode {
            DisplayMode::Mirror => SourceType::Monitor.into(),
            DisplayMode::Extend => SourceType::Virtual.into(),
        };
        let sc_proxy = Screencast::new().await?;
        sc_proxy
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_cursor_mode(CursorMode::Embedded)
                    .set_sources(requested_source)
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot),
            )
            .await?;

        // 3. Start combined RemoteDesktop session
        let response = rd_proxy
            .start(&session, None, Default::default())
            .await?
            .response()?;

        let fd = sc_proxy
            .open_pipe_wire_remote(&session, Default::default())
            .await?;

        let raw_fd = fd.as_raw_fd();
        self.pipewire_fd = Some(fd);

        let mut got_position_from_stream = false;
        let node_id = if let Some(stream) = response.streams().first() {
            let id = stream.pipe_wire_node_id();
            // Trust what the compositor actually granted over what was requested: the native
            // picker can still let the user pick something other than the app's own selection.
            if let Some(granted) = stream.source_type() {
                let resolved_mode = match granted {
                    SourceType::Monitor => DisplayMode::Mirror,
                    SourceType::Virtual => DisplayMode::Extend,
                    _ => self.display_mode,
                };
                if resolved_mode != self.display_mode {
                    tracing::info!(
                        "Portal granted source type {:?}, overriding requested display mode {:?} -> {:?}",
                        granted,
                        self.display_mode,
                        resolved_mode
                    );
                    self.display_mode = resolved_mode;
                }
            }
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
            // The virtual monitor never shows up in `DisplayConfig.GetCurrentState` (it's
            // hidden from that introspection API, apparently deliberately, even though it's a
            // real spatial part of the desktop layout - confirmed empirically: physical input
            // can cross a real monitor's edge into it). `stream.position()` is the portal's own
            // report of where the stream actually sits in the compositor's global coordinate
            // space, and is the only reliable source for this in `Extend` mode.
            if let Some((x, y)) = stream.position() {
                tracing::info!("GNOME Portal reported stream position: ({}, {})", x, y);
                self.x_offset.store(x, Ordering::SeqCst);
                self.y_offset.store(y, Ordering::SeqCst);
                got_position_from_stream = true;
            } else {
                tracing::info!("GNOME Portal did not report a stream position");
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

        if self.display_mode == DisplayMode::Extend && !got_position_from_stream {
            tracing::warn!(
                "No stream position from the portal; falling back to the DisplayConfig lookup (known not to find headless/virtual outputs on some Mutter versions)"
            );
            let _ = self.align_virtual_monitor().await;
        }

        Ok((node_id, Some(raw_fd), self.display_mode))
    }

    /// Injects absolute pointer motion coordinates onto the virtual display or mirror display.
    pub async fn notify_pointer_motion_absolute(
        &self,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
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
    async fn notify_motion_mirror(&self, norm_x: f64, norm_y: f64) -> Result<(), DaemonError> {
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
    async fn notify_motion_extend(&self, norm_x: f64, norm_y: f64) -> Result<(), DaemonError> {
        let w = self.actual_width.load(Ordering::SeqCst) as f64;
        let h = self.actual_height.load(Ordering::SeqCst) as f64;
        let px = (norm_x * w).clamp(0.0, w);
        let py = (norm_y * h).clamp(0.0, h);

        let x_off = self.x_offset.load(Ordering::SeqCst) as f64;
        let y_off = self.y_offset.load(Ordering::SeqCst) as f64;

        let abs_x = x_off + px;
        let abs_y = y_off + py;

        let (dx, dy) = {
            let mut lx = self
                .last_abs_x
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut ly = self
                .last_abs_y
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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

            match rd
                .notify_pointer_motion_absolute(session, node_id, px, py, Default::default())
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        "EXTEND MOTION: stream-relative attempt (node_id, local px/py) succeeded"
                    );
                    return Ok(());
                }
                Err(e) => tracing::info!("EXTEND MOTION: stream-relative attempt failed: {}", e),
            }
            match rd
                .notify_pointer_motion_absolute(session, node_id, abs_x, abs_y, Default::default())
                .await
            {
                Ok(()) => {
                    tracing::info!("EXTEND MOTION: node_id + global abs_x/abs_y attempt succeeded");
                    return Ok(());
                }
                Err(e) => tracing::info!(
                    "EXTEND MOTION: node_id + global abs_x/abs_y attempt failed: {}",
                    e
                ),
            }
            match rd
                .notify_pointer_motion_absolute(session, 0, abs_x, abs_y, Default::default())
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        "EXTEND MOTION: stream 0 + global abs_x/abs_y attempt succeeded"
                    );
                    return Ok(());
                }
                Err(e) => tracing::info!(
                    "EXTEND MOTION: stream 0 + global abs_x/abs_y attempt failed: {}",
                    e
                ),
            }

            // Fallback: Relative Motion for Extend Mode
            tracing::info!("EXTEND MOTION: all absolute attempts failed, falling back to relative motion (dx={:.1}, dy={:.1})", dx, dy);
            let _ = rd
                .notify_pointer_motion(session, dx, dy, Default::default())
                .await;
        }
        Ok(())
    }

    /// Injects a contextual right-click event at normalized screen coordinates.
    pub async fn notify_right_click(&self, norm_x: f64, norm_y: f64) -> Result<(), DaemonError> {
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
    pub async fn notify_overview_toggle(&self) -> Result<(), DaemonError> {
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
    pub async fn notify_scroll(&self, dx: f64, dy: f64) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            let _ = rd
                .notify_pointer_axis(session, dx, dy, Default::default())
                .await;
        }
        Ok(())
    }

    /// Converts normalized (0.0..1.0) coordinates to the stream-local pixel coordinates the
    /// portal's touch/pointer APIs expect.
    fn local_pixel(&self, norm_x: f64, norm_y: f64) -> (f64, f64) {
        let w = self.actual_width.load(Ordering::SeqCst) as f64;
        let h = self.actual_height.load(Ordering::SeqCst) as f64;
        (
            (norm_x.clamp(0.0, 1.0) * w).clamp(0.0, w),
            (norm_y.clamp(0.0, 1.0) * h).clamp(0.0, h),
        )
    }

    /// Injects a touch contact event at normalized screen coordinates via the portal's
    /// dedicated touch API (`NotifyTouchDown`). Used for real (potentially multi-point) finger
    /// touches; the stylus path uses the pointer API instead so it gets a visible cursor (see
    /// `notify_stylus_down`).
    pub async fn notify_touch_down(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let (px, py) = self.local_pixel(norm_x, norm_y);
        let node_id = self.node_id.unwrap_or(0);

        tracing::info!(
            "TOUCH DOWN slot {} -> Stream {} Pixel ({:.1}, {:.1})",
            slot,
            node_id,
            px,
            py
        );
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            if let Err(e) = rd
                .notify_touch_down(session, node_id, slot, px, py, Default::default())
                .await
            {
                tracing::error!("notify_touch_down error on slot {}: {}", slot, e);
            }
        }
        Ok(())
    }

    /// Injects touch motion movement at normalized screen coordinates via `NotifyTouchMotion`.
    pub async fn notify_touch_motion(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let (px, py) = self.local_pixel(norm_x, norm_y);
        let node_id = self.node_id.unwrap_or(0);

        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            if let Err(e) = rd
                .notify_touch_motion(session, node_id, slot, px, py, Default::default())
                .await
            {
                tracing::error!("notify_touch_motion error on slot {}: {}", slot, e);
            }
        }
        Ok(())
    }

    /// Injects a touch release event via `NotifyTouchUp`.
    pub async fn notify_touch_up(&self, slot: u32) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!("TOUCH UP slot {}", slot);
            if let Err(e) = rd.notify_touch_up(session, slot, Default::default()).await {
                tracing::error!("notify_touch_up error on slot {}: {}", slot, e);
            }
        }
        Ok(())
    }

    /// Injects a mouse pointer button event (pressed or released) via the RemoteDesktop portal.
    pub async fn notify_pointer_button(
        &self,
        button: i32,
        state: KeyState,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::info!("Injecting Pointer Button: {} {:?}", button, state);
            rd.notify_pointer_button(session, button, state, Default::default())
                .await
                .inspect_err(|e| tracing::error!("Portal notify_pointer_button error: {}", e))?;
        } else {
            tracing::warn!("notify_pointer_button skipped: rd or session missing!");
        }
        Ok(())
    }

    /// Injects active stylus down contact event via the pointer API
    /// (`notify_pointer_motion_absolute` + `notify_pointer_button`), rather than the touch API.
    /// A stylus is single-point and expects a visible cursor tracking it even before contact,
    /// which only the pointer device drives; a touchscreen contact never moves a visible cursor
    /// sprite, by design, on any compositor. The RemoteDesktop portal has no pressure/tilt-aware
    /// input path, so `pressure`/`tilt_x`/`tilt_y` are accepted (for logging/future use) but not
    /// forwarded.
    pub async fn notify_stylus_down(
        &self,
        norm_x: f64,
        norm_y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    ) -> Result<(), DaemonError> {
        tracing::info!(
            "STYLUS DOWN -> Norm ({:.4}, {:.4}) Pressure: {:.2} Tilt: ({:.1}, {:.1})",
            norm_x,
            norm_y,
            pressure,
            tilt_x,
            tilt_y
        );
        self.notify_pointer_motion_absolute(norm_x, norm_y).await?;
        self.notify_pointer_button(272, KeyState::Pressed).await
    }

    /// Injects active stylus motion event via the pointer API.
    pub async fn notify_stylus_move(
        &self,
        norm_x: f64,
        norm_y: f64,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    ) -> Result<(), DaemonError> {
        tracing::trace!(
            "STYLUS MOVE -> Norm ({:.4}, {:.4}) Pressure: {:.2} Tilt: ({:.1}, {:.1})",
            norm_x,
            norm_y,
            pressure,
            tilt_x,
            tilt_y
        );
        self.notify_pointer_motion_absolute(norm_x, norm_y).await
    }

    /// Injects active stylus lift event via the pointer API.
    pub async fn notify_stylus_up(&self, norm_x: f64, norm_y: f64) -> Result<(), DaemonError> {
        tracing::info!("STYLUS UP -> Norm ({:.4}, {:.4})", norm_x, norm_y);
        self.notify_pointer_button(272, KeyState::Released).await?;
        self.notify_pointer_motion_absolute(norm_x, norm_y).await
    }

    /// Handles continuous 1:1 swipe gestures and triggers overview toggle when threshold is met.
    pub async fn notify_swipe_gesture(&self, _dx: f64, dy: f64) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        tracing::info!("SWIPE GESTURE -> dy: {:.2}", dy);
        if dy.abs() > 30.0 {
            let _ = self.notify_overview_toggle().await;
        }
        Ok(())
    }

    /// Queries Mutter's `DisplayConfig` D-Bus interface to locate the created virtual monitor
    /// (in `Extend` mode) and cache its logical offset and resolution for coordinate mapping.
    pub async fn align_virtual_monitor(&self) -> Result<(), DaemonError> {
        let connection = zbus::Connection::session().await?;

        let reply = connection
            .call_method(
                Some("org.gnome.Mutter.DisplayConfig"),
                "/org/gnome/Mutter/DisplayConfig",
                Some("org.gnome.Mutter.DisplayConfig"),
                "GetCurrentState",
                &(),
            )
            .await?;

        let body = reply.body();
        let (serial, monitors, logical_monitors, _props): (
            u32,
            Vec<DisplayMonitor>,
            Vec<LogicalMonitor>,
            std::collections::HashMap<String, OwnedValue>,
        ) = body.deserialize().map_err(|e| {
            tracing::error!("Failed to deserialize GetCurrentState D-Bus reply: {}", e);
            DaemonError::other(format!("Failed to deserialize state: {}", e))
        })?;

        tracing::info!(
            "D-Bus DisplayConfig state: Serial {}, logical monitors count: {}",
            serial,
            logical_monitors.len()
        );

        let mut found_virtual = false;
        if self.display_mode == DisplayMode::Extend {
            for lm in &logical_monitors {
                for spec in &lm.5 {
                    tracing::info!(
                        "DisplayConfig logical monitor connector='{}' vendor='{}' product='{}' serial='{}'",
                        spec.0,
                        spec.1,
                        spec.2,
                        spec.3
                    );
                }
            }
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
    pub async fn destroy_display(&mut self) -> Result<(), DaemonError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_pixel_scales_normalized_coordinates_to_resolution() {
        let mut mgr = VirtualMonitorManager::new().await.unwrap();
        mgr.update_target_resolution(1000, 500);

        assert_eq!(mgr.local_pixel(0.0, 0.0), (0.0, 0.0));
        assert_eq!(mgr.local_pixel(1.0, 1.0), (1000.0, 500.0));
        assert_eq!(mgr.local_pixel(0.5, 0.5), (500.0, 250.0));
    }

    #[tokio::test]
    async fn local_pixel_clamps_out_of_range_coordinates() {
        let mut mgr = VirtualMonitorManager::new().await.unwrap();
        mgr.update_target_resolution(1000, 500);

        assert_eq!(mgr.local_pixel(-1.0, 2.0), (0.0, 500.0));
    }

    #[tokio::test]
    async fn update_target_resolution_ignores_zero_dimensions() {
        let mut mgr = VirtualMonitorManager::new().await.unwrap();
        mgr.update_target_resolution(800, 600);

        // A zero-valued dimension (seen from a malformed or premature handshake packet) must
        // not clobber the last good resolution.
        mgr.update_target_resolution(0, 0);
        assert_eq!(mgr.local_pixel(1.0, 1.0), (800.0, 600.0));
    }

    #[tokio::test]
    async fn notify_methods_are_safe_noops_without_an_active_session() {
        // Before `create_display` succeeds, `remote_desktop`/`session` are `None`. Every notify_*
        // method must degrade to a no-op `Ok(())` rather than panicking (e.g. on an `.unwrap()`
        // of the missing session) - this can be reached in practice if input arrives while a
        // stream is tearing down.
        let mgr = VirtualMonitorManager::new().await.unwrap();
        assert!(mgr.notify_pointer_motion_absolute(0.5, 0.5).await.is_ok());
        assert!(mgr.notify_right_click(0.5, 0.5).await.is_ok());
        assert!(mgr.notify_overview_toggle().await.is_ok());
        assert!(mgr.notify_scroll(1.0, 1.0).await.is_ok());
        assert!(mgr.notify_touch_down(0, 0.5, 0.5).await.is_ok());
        assert!(mgr.notify_touch_motion(0, 0.5, 0.5).await.is_ok());
        assert!(mgr.notify_touch_up(0).await.is_ok());
        assert!(mgr.notify_swipe_gesture(0.0, 0.0).await.is_ok());
    }

    #[tokio::test]
    async fn notify_touch_down_sets_last_input_on_virtual() {
        let mgr = VirtualMonitorManager::new().await.unwrap();
        assert!(!mgr.was_last_input_on_virtual.load(Ordering::SeqCst));
        let _ = mgr.notify_touch_down(0, 0.5, 0.5).await;
        assert!(mgr.was_last_input_on_virtual.load(Ordering::SeqCst));
    }

    #[test]
    fn daemon_error_is_cancelled_only_for_a_cancelled_portal_response() {
        let cancelled = DaemonError::from(ashpd::Error::Response(
            ashpd::desktop::ResponseError::Cancelled,
        ));
        assert!(cancelled.is_cancelled());

        let other_portal_error =
            DaemonError::from(ashpd::Error::Response(ashpd::desktop::ResponseError::Other));
        assert!(!other_portal_error.is_cancelled());

        let generic_error = DaemonError::other("something else went wrong");
        assert!(!generic_error.is_cancelled());
    }
}
