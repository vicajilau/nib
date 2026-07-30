use gtk4::prelude::*;
use gtk4::{cairo, gdk, glib};
use std::time::Duration;

/// Interval between forced repaints of the virtual monitor's stage. Bounds the worst-case
/// staleness of the cursor position shown in the screencast: the compositor only recomposes
/// (and therefore re-samples the embedded cursor sprite) when *something* damages that output's
/// stage, and virtual/headless monitors have no DRM/KMS vblank to drive that independently of
/// content changes the way a real monitor's per-frame "deadline timer" does.
const REPAINT_INTERVAL: Duration = Duration::from_millis(33);

/// Forces Mutter to keep repainting an otherwise-idle virtual monitor's stage, so the
/// screencast's embedded cursor sprite (`CursorMode::Embedded`) doesn't visibly freeze whenever
/// the pointer moves over a bare desktop with no window content to redraw.
///
/// Virtual monitors created via the portal (`Extend` mode) aren't backed by a real DRM/KMS CRTC,
/// so they don't get Mutter's independent per-vblank cursor resampling that keeps a *real*
/// monitor's screencast cursor smooth even when nothing else changes. Instead, the embedded
/// cursor is only recomposited into the captured frame when the stage repaints for some other
/// reason - a window redrawing under the pointer, or the repaint a click itself forces. On an
/// empty virtual desktop that means cursor movement is invisible in the stream until the next
/// unrelated repaint happens to catch it up.
///
/// This keeps a fullscreen, click-through, near-fully-transparent window on that output
/// animating at `REPAINT_INTERVAL`, purely to keep manufacturing that "something else redrew"
/// trigger on a steady cadence. It's a workaround for the missing compositor-side mechanism, not
/// a fix for it - and it costs a small constant amount of idle CPU/encoder work for as long as
/// an `Extend` mode stream is active.
pub struct CursorKeepaliveOverlay {
    window: gtk4::Window,
    timeout_id: Option<glib::SourceId>,
}

impl CursorKeepaliveOverlay {
    /// Locates the virtual monitor's `GdkMonitor` (matching the same connector-name heuristic
    /// used for the `DisplayConfig` D-Bus lookup in `align_virtual_monitor`) and starts the
    /// repaint overlay on it. Retries briefly since GDK's monitor list updates asynchronously
    /// after the portal creates the output. Returns `None` (logging a warning) if no matching
    /// monitor shows up in time, leaving the stream to run without the workaround.
    pub async fn start() -> Option<Self> {
        let Some(display) = gdk::Display::default() else {
            tracing::warn!("CursorKeepaliveOverlay: no default GdkDisplay available");
            return None;
        };

        let mut monitor = None;
        for _ in 0..10 {
            monitor = Self::find_virtual_monitor(&display);
            if monitor.is_some() {
                break;
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }

        let Some(monitor) = monitor else {
            tracing::warn!(
                "CursorKeepaliveOverlay: no virtual monitor found in GDK monitor list; cursor may lag on an empty virtual desktop"
            );
            return None;
        };

        let window = gtk4::Window::new();
        window.set_decorated(false);
        window.set_title(Some(""));

        let drawing_area = gtk4::DrawingArea::new();
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

        let timeout_id = glib::timeout_add_local(REPAINT_INTERVAL, move || {
            drawing_area.queue_draw();
            glib::ControlFlow::Continue
        });

        Some(Self {
            window,
            timeout_id: Some(timeout_id),
        })
    }

    /// Finds a monitor whose connector name matches Mutter's virtual/headless output naming
    /// (mirrors the heuristic used against `DisplayConfig`'s D-Bus monitor list elsewhere).
    fn find_virtual_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
        let monitors = display.monitors();
        let mut seen = Vec::new();
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok())
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
