use crate::daemon::DaemonError;
use ashpd::desktop::remote_desktop::{DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use futures_util::StreamExt;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedValue;

/// Cumulative delta (in the client's swipe-delta units) a 3-finger swipe must reach on one
/// axis before it triggers a workspace switch (horizontal) or Overview toggle (vertical).
const SWIPE_TRIGGER_THRESHOLD: f64 = 120.0;

/// Gap between consecutive `SwipeGesture` packets after which the next packet is treated as
/// the start of a new gesture rather than a continuation of the current one.
const SWIPE_GESTURE_TIMEOUT: Duration = Duration::from_millis(200);

/// Argument shape for `NotifyPointerMotionAbsolute` in `Extend` mode, cached in
/// `VirtualMonitorManager::motion_strategy` once one is known to work.
mod motion {
    /// Nothing known yet: the next motion event probes each shape in turn.
    pub const UNPROBED: u8 = 0;
    /// The stream's own node id, with stream-local pixel coordinates.
    pub const STREAM_LOCAL: u8 = 1;
    /// The stream's own node id, with desktop-global coordinates.
    pub const STREAM_GLOBAL: u8 = 2;
    /// Stream id 0, with desktop-global coordinates.
    pub const ZERO_GLOBAL: u8 = 3;
    /// No absolute shape was accepted: send relative deltas instead.
    pub const RELATIVE: u8 = 4;

    /// The absolute shapes, in the order they are probed.
    pub const ABSOLUTE_CANDIDATES: [u8; 3] = [STREAM_LOCAL, STREAM_GLOBAL, ZERO_GLOBAL];

    /// Human-readable name for logs.
    pub fn name(strategy: u8) -> &'static str {
        match strategy {
            STREAM_LOCAL => "node_id + stream-local",
            STREAM_GLOBAL => "node_id + desktop-global",
            ZERO_GLOBAL => "stream 0 + desktop-global",
            RELATIVE => "relative deltas",
            _ => "unprobed",
        }
    }
}

/// The coordinate forms a single `Extend` motion event can be expressed in, computed once per
/// event and reused across however many strategies get attempted.
struct MotionCoords {
    node_id: u32,
    local: (f64, f64),
    global: (f64, f64),
    relative: (f64, f64),
}

/// Accumulated state for the 3-finger swipe gesture currently in progress, used to fire at
/// most one action per physical gesture (see `notify_swipe_gesture`).
#[derive(Default)]
struct SwipeGestureState {
    dx_accum: f64,
    dy_accum: f64,
    last_event: Option<Instant>,
    fired: bool,
}

/// The GNOME action a completed 3-finger swipe gesture should trigger.
enum SwipeAction {
    Overview,
    WorkspaceLeft,
    WorkspaceRight,
}

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
    // `Arc`-wrapped so a background task can hold its own handle to await the `Closed` signal
    // for the session's full lifetime without borrowing from `self` (ashpd's `Session` doesn't
    // implement `Clone`, and Rust 2024's RPITIT capture rules would otherwise tie the signal
    // stream's lifetime to a short-lived local borrow).
    session: Option<Arc<Session<RemoteDesktop>>>,
    /// Whether the display session extends the desktop or mirrors an existing monitor.
    pub display_mode: DisplayMode,
    width: u32,
    height: u32,
    /// Tracks whether the most recent pointer input targeted the virtual/mirrored display,
    /// so the cursor can be reset to the primary screen on teardown.
    pub was_last_input_on_virtual: AtomicBool,
    /// Set by a background watcher when the portal session's `Closed` signal fires - e.g. the
    /// user stops sharing from GNOME Shell's own screen-sharing system indicator rather than
    /// nib's "Stop stream" button. Polled by `NibDaemon::is_session_alive` so the app notices
    /// and tears its own state down instead of staying stuck showing "streaming".
    session_closed: Arc<AtomicBool>,

    x_offset: AtomicI32,
    y_offset: AtomicI32,
    actual_width: AtomicI32,
    actual_height: AtomicI32,
    last_abs_x: Mutex<f64>,
    last_abs_y: Mutex<f64>,
    swipe_state: Mutex<SwipeGestureState>,
    /// Which `NotifyPointerMotionAbsolute` argument shape this portal accepts in `Extend`
    /// mode, one of the `motion::*` constants. Portals disagree on whether the coordinates
    /// are stream-local or desktop-global and on which stream id they want, so the working
    /// shape has to be found by trying. Caching it keeps steady-state motion at one D-Bus
    /// round trip per event instead of re-walking the whole list every time.
    motion_strategy: AtomicU8,
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
            session_closed: Arc::new(AtomicBool::new(false)),
            x_offset: AtomicI32::new(0),
            y_offset: AtomicI32::new(0),
            actual_width: AtomicI32::new(2560),
            actual_height: AtomicI32::new(1600),
            last_abs_x: Mutex::new(0.0),
            last_abs_y: Mutex::new(0.0),
            swipe_state: Mutex::new(SwipeGestureState::default()),
            motion_strategy: AtomicU8::new(motion::UNPROBED),
        })
    }

    /// Logs the connected client device's native screen resolution from its handshake,
    /// ignoring zero-valued dimensions.
    ///
    /// This must NOT touch `actual_width`/`actual_height`: those track the *captured stream's*
    /// real pixel size (set from the portal's own `stream.size()` in `create_display`), which
    /// `local_pixel` relies on to convert the client's already-normalized (0.0..1.0) touch/
    /// pointer coordinates into portal-space pixels. The client sends this handshake over the
    /// input socket only after `create_display` has already run and fixed that stream size, so
    /// overwriting it here with the device's own resolution desyncs every subsequent touch/
    /// pointer position from the real captured stream - the portal then rejects them with
    /// "Invalid position" once the device resolution differs from the stream's.
    pub fn update_target_resolution(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            tracing::info!(
                "Client device native resolution: {}x{} (Aspect Ratio: {:.2}:1)",
                width,
                height,
                width as f64 / height as f64
            );
        }
    }

    /// Reports the captured stream's logical geometry `(x, y, width, height)` in the
    /// compositor's global coordinate space, as last reported by the portal (see
    /// `create_display`). Used to locate the corresponding `GdkMonitor` for
    /// `CursorKeepaliveOverlay`, which needs to target whichever output is actually being
    /// captured - the mirrored physical monitor, or the virtual one - rather than assuming one.
    pub fn stream_geometry(&self) -> (i32, i32, i32, i32) {
        (
            self.x_offset.load(Ordering::SeqCst),
            self.y_offset.load(Ordering::SeqCst),
            self.actual_width.load(Ordering::SeqCst),
            self.actual_height.load(Ordering::SeqCst),
        )
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

        let session = Arc::new(session);

        // Watch for the session being closed by something other than nib's own
        // `destroy_display` - most notably GNOME Shell's screen-sharing system indicator,
        // whose "Turn Off" button closes the portal session directly. Without this, nib never
        // learns the stream ended and keeps showing it as active until the user manually
        // presses "Stop stream". The subscription is set up inside the spawned task itself
        // (rather than awaited here and the resulting stream handed off) so it can hold its
        // own `Arc` clone of the session for as long as it runs, instead of borrowing one that
        // only lives for the duration of this function call.
        let session_watch = session.clone();
        let flag = self.session_closed.clone();
        self.session_closed.store(false, Ordering::SeqCst);
        tokio::spawn(async move {
            match session_watch.receive_closed().await {
                Ok(mut closed_stream) => {
                    if closed_stream.next().await.is_some() {
                        tracing::info!(
                            "Portal session closed externally (e.g. system screen-sharing control)"
                        );
                        flag.store(true, Ordering::SeqCst);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to subscribe to portal session Closed signal; external stops won't be detected: {}",
                        e
                    );
                }
            }
        });

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
            tracing::trace!(
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
            let coords = MotionCoords {
                node_id: self.node_id.unwrap_or(0),
                local: (px, py),
                global: (abs_x, abs_y),
                relative: (dx, dy),
            };
            tracing::trace!(
                "EXTEND MOTION: Stream {} | Local ({:.1}, {:.1}) | Global ({:.1}, {:.1})",
                coords.node_id,
                px,
                py,
                abs_x,
                abs_y
            );

            // Fast path: whichever shape worked last time, one round trip. Only if it has
            // started failing do we fall through and probe again.
            let cached = self.motion_strategy.load(Ordering::Relaxed);
            if cached != motion::UNPROBED {
                match Self::send_motion(rd, session, cached, &coords).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(
                            "EXTEND MOTION: cached strategy ({}) stopped working: {}; re-probing",
                            motion::name(cached),
                            e
                        );
                        self.motion_strategy
                            .store(motion::UNPROBED, Ordering::Relaxed);
                    }
                }
            }

            for candidate in motion::ABSOLUTE_CANDIDATES {
                match Self::send_motion(rd, session, candidate, &coords).await {
                    Ok(()) => {
                        tracing::info!(
                            "EXTEND MOTION: portal accepted {}; caching it for this session",
                            motion::name(candidate)
                        );
                        self.motion_strategy.store(candidate, Ordering::Relaxed);
                        return Ok(());
                    }
                    Err(e) => tracing::debug!(
                        "EXTEND MOTION: portal rejected {}: {}",
                        motion::name(candidate),
                        e
                    ),
                }
            }

            tracing::info!(
                "EXTEND MOTION: no absolute shape accepted, falling back to {}",
                motion::name(motion::RELATIVE)
            );
            self.motion_strategy
                .store(motion::RELATIVE, Ordering::Relaxed);
            let _ = Self::send_motion(rd, session, motion::RELATIVE, &coords).await;
        }
        Ok(())
    }

    /// Issues one pointer-motion call in the argument shape `strategy` names. Split out so the
    /// cached fast path and the probe loop send events exactly the same way.
    async fn send_motion(
        rd: &RemoteDesktop,
        session: &Session<RemoteDesktop>,
        strategy: u8,
        coords: &MotionCoords,
    ) -> Result<(), ashpd::Error> {
        let (local_x, local_y) = coords.local;
        let (global_x, global_y) = coords.global;
        let (dx, dy) = coords.relative;
        match strategy {
            motion::STREAM_LOCAL => {
                rd.notify_pointer_motion_absolute(
                    session,
                    coords.node_id,
                    local_x,
                    local_y,
                    Default::default(),
                )
                .await
            }
            motion::STREAM_GLOBAL => {
                rd.notify_pointer_motion_absolute(
                    session,
                    coords.node_id,
                    global_x,
                    global_y,
                    Default::default(),
                )
                .await
            }
            motion::ZERO_GLOBAL => {
                rd.notify_pointer_motion_absolute(
                    session,
                    0,
                    global_x,
                    global_y,
                    Default::default(),
                )
                .await
            }
            _ => {
                rd.notify_pointer_motion(session, dx, dy, Default::default())
                    .await
            }
        }
    }

    /// Injects a contextual right-click event at normalized screen coordinates.
    pub async fn notify_right_click(&self, norm_x: f64, norm_y: f64) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        let norm_x = norm_x.clamp(0.0, 1.0);
        let norm_y = norm_y.clamp(0.0, 1.0);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::debug!(
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
            tracing::debug!("Injecting GNOME Overview Gesture (Super Key 125)...");
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

    /// Injects a touch contact event. On `Mirror` (a real, hardware-backed monitor) this uses
    /// the portal's dedicated touch API (`NotifyTouchDown`). On `Extend`, real touch is routed
    /// through the pointer API instead (like `notify_stylus_down`) because `NotifyTouchDown`/
    /// `NotifyTouchMotion` empirically fail with "Invalid position" for *any* coordinate on the
    /// headless/virtual output Extend mode captures - regardless of stream-local vs. global
    /// encoding, unlike absolute pointer motion, which Mutter accepts fine there. This trades
    /// away true multitouch (e.g. two simultaneous fingers) on `Extend` for taps/drags actually
    /// working at all, since the real touch API never succeeds there in practice.
    pub async fn notify_touch_down(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);

        if self.display_mode == DisplayMode::Extend {
            tracing::debug!(
                "TOUCH DOWN slot {} -> routed via pointer (Extend mode)",
                slot
            );
            self.notify_pointer_motion_absolute(norm_x, norm_y).await?;
            return self.notify_pointer_button(272, KeyState::Pressed).await;
        }

        let (px, py) = self.local_pixel(norm_x, norm_y);
        let node_id = self.node_id.unwrap_or(0);

        tracing::debug!(
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

    /// Injects touch motion movement. See `notify_touch_down` for why `Extend` routes through
    /// the pointer API instead of `NotifyTouchMotion`.
    pub async fn notify_touch_motion(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);

        if self.display_mode == DisplayMode::Extend {
            return self.notify_pointer_motion_absolute(norm_x, norm_y).await;
        }

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

    /// Injects a touch release event via `NotifyTouchUp` (`Mirror`), or the pointer-button
    /// release at `norm_x`/`norm_y` (`Extend`) - see `notify_touch_down`. Moves the pointer to
    /// the release position *before* releasing the button (as in `notify_stylus_up`): the
    /// portal registers a button release at wherever the pointer currently is, so releasing
    /// first would register the tap at the previous position instead of the lift-off point.
    pub async fn notify_touch_up(
        &self,
        slot: u32,
        norm_x: f64,
        norm_y: f64,
    ) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);

        if self.display_mode == DisplayMode::Extend {
            tracing::debug!("TOUCH UP slot {} -> routed via pointer (Extend mode)", slot);
            self.notify_pointer_motion_absolute(norm_x, norm_y).await?;
            return self.notify_pointer_button(272, KeyState::Released).await;
        }

        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            tracing::debug!("TOUCH UP slot {}", slot);
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
            tracing::debug!("Injecting Pointer Button: {} {:?}", button, state);
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
        tracing::debug!(
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
        tracing::debug!("STYLUS UP -> Norm ({:.4}, {:.4})", norm_x, norm_y);
        // Move first, then release: `notify_pointer_button` has no position of its own, so the
        // portal registers the release wherever the pointer currently is. Releasing before this
        // final motion would fire the click at the *previous* (last `StylusMove`) position and
        // only snap the cursor to the lift-off point afterward.
        self.notify_pointer_motion_absolute(norm_x, norm_y).await?;
        self.notify_pointer_button(272, KeyState::Released).await
    }

    /// Handles a 3-finger swipe gesture, streamed by the client as one delta packet per
    /// touch-move frame for the whole gesture. Deltas are accumulated per-axis across the
    /// gesture (reset after a gap of `SWIPE_GESTURE_TIMEOUT` with no packets, which reliably
    /// separates distinct gestures) so the dominant axis fires exactly one action -
    /// horizontal for GNOME workspace switching, vertical for the Overview toggle - instead of
    /// re-firing on every packet whose *own* delta happens to clear the threshold.
    pub async fn notify_swipe_gesture(&self, dx: f64, dy: f64) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);

        let action = {
            let mut state = self.swipe_state.lock().unwrap();
            let now = Instant::now();
            let gesture_expired = state
                .last_event
                .is_none_or(|last| now.duration_since(last) > SWIPE_GESTURE_TIMEOUT);
            if gesture_expired {
                state.dx_accum = 0.0;
                state.dy_accum = 0.0;
                state.fired = false;
            }
            state.last_event = Some(now);
            state.dx_accum += dx;
            state.dy_accum += dy;

            if state.fired {
                None
            } else if state.dy_accum.abs() > SWIPE_TRIGGER_THRESHOLD
                && state.dy_accum.abs() >= state.dx_accum.abs()
            {
                state.fired = true;
                Some(SwipeAction::Overview)
            } else if state.dx_accum.abs() > SWIPE_TRIGGER_THRESHOLD {
                state.fired = true;
                Some(if state.dx_accum > 0.0 {
                    SwipeAction::WorkspaceRight
                } else {
                    SwipeAction::WorkspaceLeft
                })
            } else {
                None
            }
        };

        match action {
            Some(SwipeAction::Overview) => {
                tracing::debug!("SWIPE GESTURE -> Overview toggle");
                self.notify_overview_toggle().await
            }
            Some(SwipeAction::WorkspaceLeft) => {
                tracing::debug!("SWIPE GESTURE -> Workspace Left");
                self.notify_workspace_switch(false).await
            }
            Some(SwipeAction::WorkspaceRight) => {
                tracing::debug!("SWIPE GESTURE -> Workspace Right");
                self.notify_workspace_switch(true).await
            }
            None => Ok(()),
        }
    }

    /// Injects GNOME's default "switch to workspace on the left/right" shortcut
    /// (Ctrl+Alt+Left/Right) via keycode simulation.
    pub async fn notify_workspace_switch(&self, right: bool) -> Result<(), DaemonError> {
        self.was_last_input_on_virtual.store(true, Ordering::SeqCst);
        if let (Some(rd), Some(session)) = (&self.remote_desktop, &self.session) {
            const KEY_LEFTCTRL: i32 = 29;
            const KEY_LEFTALT: i32 = 56;
            const KEY_LEFT: i32 = 105;
            const KEY_RIGHT: i32 = 106;
            let arrow = if right { KEY_RIGHT } else { KEY_LEFT };
            tracing::debug!(
                "Injecting GNOME Workspace Switch ({})...",
                if right { "Right" } else { "Left" }
            );
            let _ = rd
                .notify_keyboard_keycode(
                    session,
                    KEY_LEFTCTRL,
                    KeyState::Pressed,
                    Default::default(),
                )
                .await;
            let _ = rd
                .notify_keyboard_keycode(
                    session,
                    KEY_LEFTALT,
                    KeyState::Pressed,
                    Default::default(),
                )
                .await;
            let _ = rd
                .notify_keyboard_keycode(session, arrow, KeyState::Pressed, Default::default())
                .await;
            let _ = rd
                .notify_keyboard_keycode(session, arrow, KeyState::Released, Default::default())
                .await;
            let _ = rd
                .notify_keyboard_keycode(
                    session,
                    KEY_LEFTALT,
                    KeyState::Released,
                    Default::default(),
                )
                .await;
            let _ = rd
                .notify_keyboard_keycode(
                    session,
                    KEY_LEFTCTRL,
                    KeyState::Released,
                    Default::default(),
                )
                .await;
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

    /// Reports whether the portal session was closed externally (see `session_closed`).
    pub fn is_session_closed(&self) -> bool {
        self.session_closed.load(Ordering::SeqCst)
    }

    /// Tears down the portal session, resetting the cursor to the primary display origin
    /// if the virtual/mirrored display was last touched.
    pub async fn destroy_display(&mut self) -> Result<(), DaemonError> {
        tracing::info!("Destroying Display Session");
        self.session_closed.store(false, Ordering::SeqCst);
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
    async fn local_pixel_scales_normalized_coordinates_to_the_captured_stream_resolution() {
        // `local_pixel` scales against `actual_width`/`actual_height` as seeded by `new()` (and
        // later overwritten by `create_display` from the portal's reported stream size) - never
        // against the client device's own native resolution.
        let mgr = VirtualMonitorManager::new().await.unwrap();

        assert_eq!(mgr.local_pixel(0.0, 0.0), (0.0, 0.0));
        assert_eq!(mgr.local_pixel(1.0, 1.0), (2560.0, 1600.0));
        assert_eq!(mgr.local_pixel(0.5, 0.5), (1280.0, 800.0));
    }

    #[tokio::test]
    async fn local_pixel_clamps_out_of_range_coordinates() {
        let mgr = VirtualMonitorManager::new().await.unwrap();

        assert_eq!(mgr.local_pixel(-1.0, 2.0), (0.0, 1600.0));
    }

    #[tokio::test]
    async fn update_target_resolution_does_not_affect_the_captured_stream_pixel_space() {
        // Regression test: the client's native resolution (learned from the `InitResolution`
        // handshake, which arrives over the input socket *after* `create_display` already fixed
        // the real stream/display pixel size via the portal) must never clobber `actual_width`/
        // `actual_height` - doing so previously desynced touch/pointer coordinate math from the
        // real captured stream, producing portal "Invalid position" errors.
        let mut mgr = VirtualMonitorManager::new().await.unwrap();

        mgr.update_target_resolution(1080, 2400);
        assert_eq!(mgr.local_pixel(1.0, 1.0), (2560.0, 1600.0));
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
        assert!(mgr.notify_touch_up(0, 0.5, 0.5).await.is_ok());
        assert!(mgr.notify_swipe_gesture(0.0, 0.0).await.is_ok());
    }

    #[tokio::test]
    async fn swipe_gesture_fires_at_most_once_per_continuous_gesture() {
        // Regression test: a real 3-finger swipe streams many delta packets per gesture, and
        // several consecutive packets can each individually clear a naive per-packet
        // threshold, which used to toggle the Overview open and immediately closed again.
        // The cumulative accumulator must only cross the threshold - and thus "fire" - once.
        let mgr = VirtualMonitorManager::new().await.unwrap();

        for _ in 0..3 {
            assert!(mgr.notify_swipe_gesture(0.0, 50.0).await.is_ok());
        }
        assert!(
            mgr.swipe_state.lock().unwrap().fired,
            "cumulative dy (150) should have crossed the trigger threshold"
        );

        // Further packets in the same gesture must not re-arm/re-fire.
        for _ in 0..3 {
            assert!(mgr.notify_swipe_gesture(0.0, 50.0).await.is_ok());
        }
        assert!(mgr.swipe_state.lock().unwrap().fired);
    }

    #[tokio::test]
    async fn swipe_gesture_rearms_after_a_gap_between_gestures() {
        let mgr = VirtualMonitorManager::new().await.unwrap();

        for _ in 0..3 {
            assert!(mgr.notify_swipe_gesture(0.0, 50.0).await.is_ok());
        }
        assert!(mgr.swipe_state.lock().unwrap().fired);

        // Simulate the packet stream going quiet for longer than `SWIPE_GESTURE_TIMEOUT`,
        // as happens between two separate physical swipes.
        {
            let mut state = mgr.swipe_state.lock().unwrap();
            state.last_event = Some(Instant::now() - Duration::from_millis(500));
        }

        assert!(mgr.notify_swipe_gesture(0.0, 10.0).await.is_ok());
        let state = mgr.swipe_state.lock().unwrap();
        assert!(!state.fired, "a new gesture should be able to fire again");
        assert_eq!(
            state.dy_accum, 10.0,
            "accumulator should reset on a new gesture"
        );
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
