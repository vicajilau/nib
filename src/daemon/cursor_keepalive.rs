use crate::daemon::virtual_monitor::DisplayMode;
use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib};
use std::time::Duration;

/// Interval between forced repaints of the virtual monitor's stage. Bounds the worst-case
/// staleness of the cursor position shown in the screencast: the compositor only recomposes
/// (and therefore re-samples the embedded cursor sprite) when *something* damages that output's
/// stage, and virtual/headless monitors have no DRM/KMS vblank to drive that independently of
/// content changes the way a real monitor's per-frame "deadline timer" does.
const REPAINT_INTERVAL: Duration = Duration::from_millis(33);

/// Forces Mutter to keep repainting an otherwise-idle captured monitor's stage, so the
/// screencast's embedded cursor sprite (`CursorMode::Embedded`) - and on at least this NVIDIA
/// setup, injected `RemoteDesktop` pointer/touch/scroll input generally - doesn't visibly freeze
/// the stream whenever nothing else happens to trigger a repaint.
///
/// Virtual monitors created via the portal (`Extend` mode) aren't backed by a real DRM/KMS CRTC,
/// so they don't get Mutter's independent per-vblank resampling that keeps a *real* monitor's
/// screencast smooth even when nothing else changes. A *real*, mirrored monitor (`Mirror` mode)
/// does have that vblank timer in general, but empirically (on this NVIDIA setup, at least)
/// input injected through the `RemoteDesktop` portal - as opposed to genuine hardware input -
/// doesn't reliably count as "damage" that schedules a repaint either, so `Mirror` mode streams
/// can go just as stale as `Extend` ones whenever every input event driving the stream is
/// injected rather than physical. Either way the captured frame is only recomposited when the
/// stage repaints for some other reason - a window redrawing, or the repaint a physical click
/// itself forces - so on an idle desktop driven purely by injected input, changes are invisible
/// in the stream until the next unrelated repaint happens to catch them up.
///
/// This keeps a fullscreen, click-through, near-fully-transparent window on the captured output
/// animating at `REPAINT_INTERVAL`, purely to keep manufacturing that "something else redrew"
/// trigger on a steady cadence. It's a workaround for the missing compositor-side mechanism, not
/// a fix for it - and it costs a small constant amount of idle CPU/encoder work for as long as
/// the stream is active.
pub struct CursorKeepaliveOverlay {
    window: gtk4::Window,
    timeout_id: Option<glib::SourceId>,
}

impl CursorKeepaliveOverlay {
    /// Locates the captured output's `GdkMonitor` and starts the repaint overlay on it.
    /// `mode` picks which heuristic to try first - the virtual-connector match for `Extend`,
    /// geometry match for `Mirror` - falling back to the other if the first comes up empty, since
    /// either can in principle apply (e.g. a portal that doesn't report a stream position at all
    /// forces `Extend` onto the connector heuristic; a mirrored monitor could in theory be found
    /// by the connector heuristic too if it happened to be misnamed). `stream_rect` is the
    /// captured stream's logical `(x, y, width, height)` from
    /// `VirtualMonitorManager::stream_geometry`. Retries briefly since GDK's monitor list updates
    /// asynchronously after the portal creates/selects the output. Returns `None` (logging a
    /// warning) if no matching monitor shows up in time, leaving the stream to run without the
    /// workaround.
    pub async fn start(mode: DisplayMode, stream_rect: (i32, i32, i32, i32)) -> Option<Self> {
        let Some(display) = gdk::Display::default() else {
            tracing::warn!("CursorKeepaliveOverlay: no default GdkDisplay available");
            return None;
        };

        let (x, y, w, h) = stream_rect;
        let center = (x + w / 2, y + h / 2);

        let mut monitor = None;
        for _ in 0..10 {
            monitor = match mode {
                DisplayMode::Extend => Self::find_virtual_monitor(&display)
                    .or_else(|| Self::find_monitor_by_point(&display, center)),
                DisplayMode::Mirror => Self::find_monitor_by_point(&display, center)
                    .or_else(|| Self::find_virtual_monitor(&display)),
            };
            if monitor.is_some() {
                break;
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }

        let Some(monitor) = monitor else {
            tracing::warn!(
                "CursorKeepaliveOverlay: no monitor found at captured stream geometry {:?} in GDK monitor list; cursor/content may lag on an idle desktop",
                stream_rect
            );
            return None;
        };

        let geom = monitor.geometry();
        tracing::info!(
            "CursorKeepaliveOverlay: targeting monitor connector={:?} model={:?} geometry=({}, {}, {}, {}) for stream_rect={:?} (mode={:?})",
            monitor.connector(),
            monitor.model(),
            geom.x(),
            geom.y(),
            geom.width(),
            geom.height(),
            stream_rect,
            mode
        );

        let window = gtk4::Window::new();
        window.set_decorated(false);
        window.set_title(Some(""));
        // Belt-and-braces alongside the child's hexpand/vexpand: gives the window itself a
        // concrete natural size up front instead of relying entirely on the fullscreen request
        // (asynchronous under Wayland) to establish one.
        window.set_default_size(geom.width(), geom.height());

        // The theme still paints an opaque background behind the drawing area's near-transparent
        // (1/255 alpha) fill, so without this the overlay shows up as a solid gray rectangle
        // instead of being effectively invisible.
        window.add_css_class("nib-cursor-keepalive");
        let css_provider = gtk4::CssProvider::new();
        css_provider
            .load_from_string("window.nib-cursor-keepalive { background-color: transparent; }");
        gtk4::style_context_add_provider_for_display(
            &display,
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // This window must never be user-closable - e.g. via GNOME Shell's Overview, which
        // offers a close affordance on every window thumbnail regardless of decoration/opacity.
        // Closing it silently drops the repaint workaround without the daemon noticing.
        window.connect_close_request(|_| glib::Propagation::Stop);

        let drawing_area = gtk4::DrawingArea::new();
        // Without these, a content-less DrawingArea has no natural size of its own; as the sole
        // child of a window forced to the monitor's full fullscreen size, it would collapse to a
        // 0x0 allocation instead of filling the window - leaving nothing for `queue_draw` to
        // actually paint, so it can never contribute real damage for the compositor to pick up
        // (confirmed via GNOME Shell logging "actor ... needs an allocation" on every tick).
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);
        drawing_area.set_draw_func(|_, cr, width, height| {
            // Effectively invisible (1/255 alpha) but non-zero, so the compositor treats this
            // as real content to composite rather than culling it as fully transparent.
            cr.set_source_rgba(0.0, 0.0, 0.0, 1.0 / 255.0);
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            let _ = cr.fill();
        });
        window.set_child(Some(&drawing_area));

        let window_for_realize = window.clone();
        window.connect_realize(move |_| {
            // Click/touch-through: this window exists purely to force repaints and must never
            // steal input meant for whatever the user actually has open on the virtual monitor.
            if let Some(surface) = window_for_realize.surface() {
                let empty_region = cairo::Region::create();
                surface.set_input_region(Some(&empty_region));
            }
        });

        window.fullscreen_on_monitor(&monitor);
        window.present();
        tracing::info!("CursorKeepaliveOverlay: overlay window presented and fullscreened");

        let timeout_id = glib::timeout_add_local(REPAINT_INTERVAL, move || {
            drawing_area.queue_draw();
            glib::ControlFlow::Continue
        });

        Some(Self {
            window,
            timeout_id: Some(timeout_id),
        })
    }

    /// Finds the `GdkMonitor` whose geometry contains `point` - the captured stream's logical
    /// center, from `VirtualMonitorManager::stream_geometry` - for locating a *real*, mirrored
    /// monitor. The portal reports stream position/size in the same logical/global coordinate
    /// space Mutter exposes to GDK (this codebase already relies on that equivalence elsewhere,
    /// e.g. mixing portal-reported and `DisplayConfig`-reported offsets interchangeably), so a
    /// straight containment check is enough - no DPI/scale conversion needed. GDK4 dropped GDK3's
    /// `gdk_display_get_monitor_at_point` convenience getter, so this scans `Display::monitors`
    /// itself instead.
    fn find_monitor_by_point(display: &gdk::Display, point: (i32, i32)) -> Option<gdk::Monitor> {
        let (px, py) = point;
        let monitors = display.monitors();
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors
                .item(i)
                .and_then(|o| o.downcast::<gdk::Monitor>().ok())
            else {
                continue;
            };
            let geom = monitor.geometry();
            if px >= geom.x()
                && px < geom.x() + geom.width()
                && py >= geom.y()
                && py < geom.y() + geom.height()
            {
                return Some(monitor);
            }
        }
        None
    }

    /// Finds a monitor whose connector name matches Mutter's virtual/headless output naming
    /// (mirrors the heuristic used against `DisplayConfig`'s D-Bus monitor list elsewhere).
    fn find_virtual_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
        let monitors = display.monitors();
        let mut seen = Vec::new();
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors
                .item(i)
                .and_then(|o| o.downcast::<gdk::Monitor>().ok())
            else {
                continue;
            };
            let connector = monitor.connector();
            seen.push(format!(
                "{}/{}",
                connector.as_deref().unwrap_or("?"),
                monitor.model().as_deref().unwrap_or("?")
            ));
            if let Some(connector) = &connector {
                if connector.starts_with("Meta") || connector.contains("Virtual") {
                    return Some(monitor);
                }
            }
        }
        tracing::info!("GDK monitor connectors seen this attempt: {:?}", seen);
        None
    }

    /// Stops the repaint loop and tears down the overlay window.
    pub fn stop(mut self) {
        if let Some(id) = self.timeout_id.take() {
            id.remove();
        }
        self.window.close();
    }
}
