pub mod adb_transport;
pub mod cursor_keepalive;
pub mod error;
pub mod input_injector;
pub mod screencast;
pub mod virtual_monitor;

use std::sync::Arc;
use tokio::sync::Mutex;

use adb_transport::AdbTransportManager;
use cursor_keepalive::CursorKeepaliveOverlay;
pub use error::DaemonError;
use input_injector::{InputInjector, InputServer};
use screencast::ScreencastPipeline;
use virtual_monitor::{DisplayMode, VirtualMonitorManager};

/// Android-side ports that `nib`'s companion app listens on; `adb reverse` maps these to the
/// per-device local ports chosen in `StreamConfig`.
const DEVICE_VIDEO_PORT: u16 = 6000;
const DEVICE_INPUT_PORT: u16 = 6001;

/// User-configurable parameters describing how a single device stream should be captured,
/// encoded, and transported.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Target virtual/mirrored display width in pixels.
    pub width: u32,
    /// Target virtual/mirrored display height in pixels.
    pub height: u32,
    /// Target encoder framerate.
    pub fps: u32,
    /// Preferred GStreamer H.264 encoder identifier hint (e.g. `"vaapi"`).
    pub encoder: String,
    /// Whether to extend the desktop with a new virtual monitor or mirror an existing one.
    pub display_mode: DisplayMode,
    /// ADB serial number of the target device, or `None` to use the default/only device.
    pub device_serial: Option<String>,
    /// Local TCP port used for the outgoing H.264 video stream.
    pub video_port: u16,
    /// Local TCP port used for the incoming input event stream.
    pub input_port: u16,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            width: 2560,
            height: 1600,
            fps: 60,
            encoder: "vaapi".to_string(),
            display_mode: DisplayMode::Extend,
            device_serial: None,
            video_port: DEVICE_VIDEO_PORT,
            input_port: DEVICE_INPUT_PORT,
        }
    }
}

/// Orchestrates a single device's end-to-end streaming session: ADB port forwarding, the
/// virtual/mirrored display session, the input event server, and the GStreamer screencast pipeline.
pub struct NibDaemon {
    config: StreamConfig,
    vm_manager: Option<Arc<Mutex<VirtualMonitorManager>>>,
    pipeline: Option<ScreencastPipeline>,
    input_server: Option<InputServer>,
    /// Forces periodic repaints on an `Extend`-mode virtual monitor so the screencast's
    /// embedded cursor doesn't freeze on an otherwise-idle desktop. Lives here (not on
    /// `VirtualMonitorManager`) because it wraps a GTK window, which isn't `Send`, while
    /// `VirtualMonitorManager` is shared into the input server's `tokio::spawn` task and must
    /// stay thread-safe; `NibDaemon` itself never leaves the GTK main thread.
    cursor_keepalive: Option<CursorKeepaliveOverlay>,
    /// Whether a stream is currently active for this daemon instance.
    pub is_streaming: bool,
}

impl NibDaemon {
    /// Creates a new, inactive `NibDaemon` with the given stream configuration.
    pub fn new(config: StreamConfig) -> Self {
        Self {
            config,
            vm_manager: None,
            pipeline: None,
            input_server: None,
            cursor_keepalive: None,
            is_streaming: false,
        }
    }

    /// Starts the full streaming pipeline: ADB port forwarding, portal display session
    /// creation, input server, and GStreamer screencast, in that order.
    pub async fn start_stream(&mut self) -> Result<(), DaemonError> {
        let video_port = self.config.video_port;
        let input_port = self.config.input_port;

        tracing::info!(
            "Starting Nib Display Stream for device {:?} on ports (Video: {}, Input: {}) Mode: {:?}, Res: {}x{}@{}fps...",
            self.config.device_serial,
            video_port,
            input_port,
            self.config.display_mode,
            self.config.width,
            self.config.height,
            self.config.fps
        );

        // 1. ADB Port Forwarding: map the device's fixed video/input ports to this stream's
        // unique local PC ports.
        let _ = AdbTransportManager::setup_port_forwarding(
            self.config.device_serial.as_deref(),
            video_port,
            DEVICE_VIDEO_PORT,
        );
        let _ = AdbTransportManager::setup_port_forwarding(
            self.config.device_serial.as_deref(),
            input_port,
            DEVICE_INPUT_PORT,
        );

        // 2. Mutter / Freedesktop Portal Display Session
        let mut vm_mgr = VirtualMonitorManager::new().await?;

        let (node_id, fd, resolved_mode) = vm_mgr
            .create_display(
                self.config.display_mode,
                self.config.width,
                self.config.height,
            )
            .await?;

        // The native portal picker can grant a different source type than what was requested
        // (e.g. the user picks a real screen in the dialog while nib's own settings said
        // Extend); trust what was actually granted for everything downstream.
        if resolved_mode != self.config.display_mode {
            tracing::info!(
                "Resolved display mode differs from requested: {:?} -> {:?}",
                self.config.display_mode,
                resolved_mode
            );
            self.config.display_mode = resolved_mode;
        }

        if self.config.display_mode == DisplayMode::Extend {
            self.cursor_keepalive = CursorKeepaliveOverlay::start().await;
        }

        let vm_mgr_arc = Arc::new(Mutex::new(vm_mgr));
        self.vm_manager = Some(vm_mgr_arc.clone());

        // 3. Input Listener Server (connected to RemoteDesktop portal)
        let injector = Arc::new(InputInjector::new(
            self.config.width,
            self.config.height,
            Some(vm_mgr_arc.clone()),
        )?);
        let input_server = InputServer::start(injector, input_port);
        self.input_server = Some(input_server);

        // 4. GStreamer PipeWire screencast pipeline targeted at display node_id and open PipeWire FD!
        let mut pipeline = ScreencastPipeline::new(self.config.clone());
        pipeline.start(video_port, node_id, fd)?;
        self.pipeline = Some(pipeline);

        self.is_streaming = true;
        Ok(())
    }

    /// Reports whether this stream's portal session is still open. `false` means it was closed
    /// by something other than `stop_stream` - most notably GNOME Shell's own screen-sharing
    /// system indicator - so the caller should tear the daemon down even though `is_streaming`
    /// is still `true`.
    ///
    /// Uses a non-blocking `try_lock` since this is meant to be polled inline from synchronous
    /// UI code; a momentarily contended lock (e.g. a concurrent teardown already in flight) is
    /// treated as "still alive" rather than blocking the caller.
    pub fn is_session_alive(&self) -> bool {
        match &self.vm_manager {
            Some(vm_mgr_arc) => match vm_mgr_arc.try_lock() {
                Ok(vm_mgr) => !vm_mgr.is_session_closed(),
                Err(_) => true,
            },
            None => true,
        }
    }

    /// Stops the input server, screencast pipeline, and portal display session, in that order.
    pub async fn stop_stream(&mut self) {
        tracing::info!("Stopping Nib Display Stream...");
        if let Some(overlay) = self.cursor_keepalive.take() {
            overlay.stop();
        }
        if let Some(mut server) = self.input_server.take() {
            server.stop();
        }
        if let Some(mut pipeline) = self.pipeline.take() {
            pipeline.stop();
        }
        if let Some(vm_mgr_arc) = self.vm_manager.take() {
            let mut vm_mgr = vm_mgr_arc.lock().await;
            let _ = vm_mgr.destroy_display().await;
        }
        self.is_streaming = false;
    }
}
