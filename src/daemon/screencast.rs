use crate::daemon::{DaemonError, StreamConfig};
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSinkCallbacks};
use std::io::Write;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// Manages low-latency H.264 hardware-accelerated video streaming pipelines using GStreamer.
pub struct ScreencastPipeline {
    #[allow(dead_code)]
    config: StreamConfig,
    pipeline: Option<gstreamer::Pipeline>,
    stop_signal: Option<Arc<AtomicBool>>,
}

impl ScreencastPipeline {
    /// Creates a new `ScreencastPipeline` instance initialized with the specified streaming parameters.
    pub fn new(config: StreamConfig) -> Self {
        Self {
            config,
            pipeline: None,
            stop_signal: None,
        }
    }

    /// Builds `key=value` property assignments for only the properties `element_name` actually
    /// exposes, so a hardcoded property list doesn't break against older/newer plugin builds
    /// (e.g. bundled in a Flatpak runtime) that don't support every property.
    fn supported_properties(element_name: &str, desired: &[(&str, &str)]) -> String {
        let Ok(element) = gstreamer::ElementFactory::make(element_name).build() else {
            return String::new();
        };
        desired
            .iter()
            .filter(|(key, _)| element.has_property(key))
            .map(|(key, value)| format!("{}={}", key, value))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether *this* process is actually confined (Flatpak).
    fn is_running_confined() -> bool {
        std::path::Path::new("/.flatpak-info").exists()
    }

    /// Auto-detects and returns configured H.264 encoder string with zero-latency parameters.
    ///
    /// Hardware encoders are skipped when running sandboxed (Flatpak): under confinement the
    /// app only gets Mesa/open-source GPU libraries, not the host's proprietary NVIDIA driver
    /// that nvh264enc/vah264enc need, so the encoder element loads but silently produces no
    /// output. Software encoding has no such dependency and always works.
    fn detect_encoder_pipeline_string() -> String {
        let sandboxed = Self::is_running_confined();
        if sandboxed {
            tracing::info!(
                "Running sandboxed: skipping hardware encoders, which need proprietary GPU driver access that isn't reliably available under confinement"
            );
        }
        if !sandboxed && gstreamer::ElementFactory::find("nvh264enc").is_some() {
            tracing::info!("Selected NVIDIA NVENC Hardware Encoder (nvh264enc) [Zero Latency]");
            let props = Self::supported_properties(
                "nvh264enc",
                &[
                    ("zerolatency", "true"),
                    ("tune", "ultra-low-latency"),
                    ("rc-mode", "cbr"),
                    ("bitrate", "12000"),
                    ("gop-size", "30"),
                    ("bframes", "0"),
                ],
            );
            format!("nvh264enc {}", props)
        } else if !sandboxed && gstreamer::ElementFactory::find("vah264enc").is_some() {
            tracing::info!("Selected VA-API Hardware Encoder (vah264enc)");
            let props = Self::supported_properties(
                "vah264enc",
                &[
                    ("rate-control", "cbr"),
                    ("bitrate", "12000"),
                    ("gop-size", "30"),
                    ("bframes", "0"),
                ],
            );
            format!("vah264enc {}", props)
        } else if !sandboxed && gstreamer::ElementFactory::find("vaapih264enc").is_some() {
            tracing::info!("Selected VA-API Hardware Encoder (vaapih264enc)");
            let props = Self::supported_properties(
                "vaapih264enc",
                &[
                    ("rate-control", "cbr"),
                    ("bitrate", "12000"),
                    ("keyframe-period", "30"),
                ],
            );
            format!("vaapih264enc {}", props)
        } else if gstreamer::ElementFactory::find("x264enc").is_some() {
            tracing::info!("Selected x264 Software Encoder (x264enc) [Zero Latency]");
            let props = Self::supported_properties(
                "x264enc",
                &[
                    ("tune", "zerolatency"),
                    ("speed-preset", "ultrafast"),
                    ("bitrate", "12000"),
                    ("key-int-max", "30"),
                    ("bframes", "0"),
                    ("threads", "4"),
                    ("sync-lookahead", "0"),
                    ("rc-lookahead", "0"),
                ],
            );
            format!("x264enc {}", props)
        } else {
            tracing::info!("Selected OpenH264 Software Encoder (openh264enc)");
            let props = Self::supported_properties(
                "openh264enc",
                &[("gop-size", "30"), ("bitrate", "12000000")],
            );
            format!("openh264enc {}", props)
        }
    }

    /// Constructs GStreamer pipeline string for PipeWire src -> H.264 enc -> AppSink.
    pub fn build_pipeline_desc(&self, node_id: Option<u32>, fd: Option<i32>) -> String {
        let encoder_str = Self::detect_encoder_pipeline_string();
        // keepalive-time resends the last buffer periodically so a static (damage-driven,
        // "on demand") screencast source doesn't stall the pipeline. Disabled (0) when
        // sandboxed: the org.gnome.Platform-bundled pipewiresrc has a known bug where the
        // *first* buffer is unreffed twice as soon as keepalive-time is nonzero
        // (`gst_mini_object_unref: assertion 'mini_object != NULL' failed`), corrupting the
        // only keyframe h264parse's config-interval=1 relies on and leaving the client with a
        // permanently black decode target. The host's system pipewire (e.g. Ubuntu's
        // gstreamer1.0-pipewire package) already carries the fix, so cargo run is unaffected.
        let keepalive = if Self::is_running_confined() { 0 } else { 16 };
        let src = match (fd, node_id) {
            (Some(fd_val), Some(id)) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time={} fd={} path={}",
                keepalive, fd_val, id
            ),
            (Some(fd_val), None) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time={} fd={}",
                keepalive, fd_val
            ),
            (None, Some(id)) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time={} path={}",
                keepalive, id
            ),
            (None, None) => {
                format!(
                    "pipewiresrc do-timestamp=true always-copy=true keepalive-time={}",
                    keepalive
                )
            }
        };
        let caps = format!(
            "video/x-raw,width={},height={}",
            self.config.width, self.config.height
        );
        // No framerate constraint anywhere in this pipeline: the portal's screencast node is
        // damage-driven (its actual framerate is 0/1, "on demand"). pipewiresrc negotiates by
        // querying the *entire* downstream chain's caps transitively, so a fixed framerate
        // constraint anywhere - even several elements downstream, after videoconvert/videoscale
        // - still fails negotiation outright ("stream error: no more input formats"), because
        // PipeWire can't resolve a fixed clocked rate against a node that only ever offers
        // 0/1. Confirmed by two separate real failed runs. `self.config.fps` (the UI's 60/120/30
        // selector) can't currently be honored this way; do-timestamp=true stamps real
        // wall-clock PTS on each buffer regardless of the nominal declared rate.
        //
        // The plain (no memory-feature) "video/x-raw" caps right after pipewiresrc rule out
        // DMA-BUF for this negotiation, forcing system-memory buffers. Under sandboxing the
        // Flatpak-bundled Mesa version doesn't match the host compositor's, and DMA-BUF
        // modifier negotiation between the two silently produces black frames instead of an
        // error. System memory has no such cross-version dependency, at the cost of an extra
        // copy that videoconvert would need to do anyway.
        // Different H.264 encoder elements default to different NAL framings: nvh264enc/
        // vah264enc emit Annex-B (start-code-delimited), but x264enc (the software fallback
        // used whenever sandboxing forces us off hardware encoders) defaults to AVC
        // (length-prefixed) instead. The Android client's MediaCodec decoder is fed this
        // stream directly and only understands Annex-B, so without forcing the format here,
        // switching encoders silently swaps the wire format and the client can't decode a
        // single frame - it just never renders anything, with no error on either side.
        //
        // Likewise, x264enc auto-negotiates its H.264 *profile* from whatever pixel format
        // videoconvert hands it, which can land on High 4:4:4 Predictive (profile_idc 244) -
        // a profile almost no Android hardware MediaCodec decoder implements. nvh264enc
        // defaults to Baseline (profile_idc 66, universally supported), so this only bites
        // the sandboxed software-encoder fallback. Forcing baseline here costs negligible
        // quality at this bitrate/resolution and guarantees the client can always decode it.
        format!(
            "{} ! video/x-raw ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! videoconvert n-threads=4 ! videoscale ! {} ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! {} ! video/x-h264,profile=baseline ! h264parse config-interval=1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=sink sync=false max-buffers=1 drop=true",
            src, caps, encoder_str
        )
    }

    /// Launches the GStreamer pipeline and binds local TCP socket listener for video stream delivery.
    pub fn start(
        &mut self,
        port: u16,
        node_id: Option<u32>,
        fd: Option<i32>,
    ) -> Result<(), DaemonError> {
        gstreamer::init()
            .map_err(|e| DaemonError::other(format!("Failed to init GStreamer: {}", e)))?;

        let pipeline_str = self.build_pipeline_desc(node_id, fd);
        tracing::info!("Launching GStreamer Screencast Pipeline: {}", pipeline_str);

        let element = gstreamer::parse::launch(&pipeline_str)
            .map_err(|e| DaemonError::other(format!("Failed to parse pipeline: {}", e)))?;

        let pipeline = element
            .dynamic_cast::<gstreamer::Pipeline>()
            .map_err(|_| DaemonError::other("Element is not a Pipeline"))?;

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| DaemonError::other("Failed to find appsink element"))?
            .dynamic_cast::<AppSink>()
            .map_err(|_| DaemonError::other("Element 'sink' is not an AppSink"))?;

        let active_stream: Arc<Mutex<Option<std::net::TcpStream>>> = Arc::new(Mutex::new(None));
        let active_stream_listener = active_stream.clone();

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_signal_thread = stop_signal.clone();

        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))?;
        listener.set_nonblocking(true)?;

        thread::spawn(move || {
            while !stop_signal_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        tracing::info!("Video stream client connected from {}", addr);
                        let _ = stream.set_nodelay(true);
                        let mut guard = active_stream_listener
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        *guard = Some(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(e) => {
                        tracing::warn!("TCP listener error: {}", e);
                        thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        });

        let active_stream_sink = active_stream.clone();
        appsink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let sample = appsink
                        .pull_sample()
                        .map_err(|_| gstreamer::FlowError::Error)?;
                    let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;
                    let map = buffer
                        .map_readable()
                        .map_err(|_| gstreamer::FlowError::Error)?;
                    let slice = map.as_slice();

                    let Ok(mut guard) = active_stream_sink.try_lock() else {
                        return Ok(gstreamer::FlowSuccess::Ok);
                    };
                    if let Some(stream) = guard.as_mut() {
                        let len = slice.len() as u32;
                        let len_bytes = len.to_be_bytes();
                        if stream.write_all(&len_bytes).is_err()
                            || stream.write_all(slice).is_err()
                            || stream.flush().is_err()
                        {
                            tracing::warn!(
                                "Failed to write frame to video TCP stream; client disconnected"
                            );
                            *guard = None;
                        }
                    }
                    Ok(gstreamer::FlowSuccess::Ok)
                })
                .build(),
        );

        if let Err(e) = pipeline.set_state(gstreamer::State::Playing) {
            if let Some(bus) = pipeline.bus() {
                if let Some(msg) = bus.pop_filtered(&[gstreamer::MessageType::Error]) {
                    if let gstreamer::MessageView::Error(err) = msg.view() {
                        let err_msg = format!(
                            "GStreamer Error from {}: {} ({:?})",
                            err.src().map(|s| s.name().to_string()).unwrap_or_default(),
                            err.error(),
                            err.debug()
                        );
                        tracing::error!("{}", err_msg);
                        let _ = pipeline.set_state(gstreamer::State::Null);
                        return Err(DaemonError::other(err_msg));
                    }
                }
            }
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Err(DaemonError::other(format!(
                "Failed to set pipeline to Playing: {}",
                e
            )));
        }

        self.pipeline = Some(pipeline);
        self.stop_signal = Some(stop_signal);
        Ok(())
    }

    /// Stops the GStreamer pipeline and releases all video buffers and TCP socket resources.
    pub fn stop(&mut self) {
        if let Some(signal) = self.stop_signal.take() {
            signal.store(true, Ordering::SeqCst);
        }
        if let Some(pipeline) = self.pipeline.take() {
            tracing::info!("Stopping Screencast Pipeline");
            let _ = pipeline.set_state(gstreamer::State::Null);
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_gst() {
        let _ = gstreamer::init();
    }

    #[test]
    fn supported_properties_filters_to_what_the_element_exposes() {
        init_gst();
        // "identity" is a core GStreamer element guaranteed to exist and to expose "silent"
        // (a bool property) but not a made-up property name.
        let props = ScreencastPipeline::supported_properties(
            "identity",
            &[("silent", "true"), ("not-a-real-property", "42")],
        );
        assert!(
            props.contains("silent=true"),
            "expected a supported property to be kept, got: {props}"
        );
        assert!(
            !props.contains("not-a-real-property"),
            "expected an unsupported property to be filtered out, got: {props}"
        );
    }

    #[test]
    fn supported_properties_empty_for_nonexistent_element() {
        init_gst();
        let props = ScreencastPipeline::supported_properties(
            "this-element-does-not-exist",
            &[("foo", "bar")],
        );
        assert_eq!(props, "");
    }

    #[test]
    fn is_running_confined_false_outside_flatpak() {
        // The test/CI environment never has /.flatpak-info; this guards the sandbox-detection
        // heuristic (see the git history for how many times it's been wrong) against silently
        // flipping to "always sandboxed" or "always native".
        assert!(!ScreencastPipeline::is_running_confined());
    }

    #[test]
    fn build_pipeline_desc_includes_node_id_and_fd_in_source() {
        init_gst();
        let config = StreamConfig {
            width: 1920,
            height: 1080,
            ..StreamConfig::default()
        };
        let pipeline = ScreencastPipeline::new(config);
        let desc = pipeline.build_pipeline_desc(Some(42), Some(7));
        assert!(desc.contains("fd=7"), "missing fd in: {desc}");
        assert!(desc.contains("path=42"), "missing node_id path in: {desc}");
    }

    #[test]
    fn build_pipeline_desc_omits_fd_and_path_when_absent() {
        init_gst();
        let pipeline = ScreencastPipeline::new(StreamConfig::default());
        let desc = pipeline.build_pipeline_desc(None, None);
        assert!(!desc.contains("fd="), "unexpected fd in: {desc}");
        assert!(!desc.contains("path="), "unexpected path in: {desc}");
    }

    #[test]
    fn build_pipeline_desc_has_no_fixed_framerate_constraint() {
        init_gst();
        // Regression guard: a fixed framerate anywhere in this pipeline breaks caps negotiation
        // against the portal's damage-driven (0/1) screencast node - see the long comment on
        // `build_pipeline_desc` for the two confirmed failure modes this avoids.
        let config = StreamConfig {
            fps: 120,
            ..StreamConfig::default()
        };
        let pipeline = ScreencastPipeline::new(config);
        let desc = pipeline.build_pipeline_desc(None, None);
        assert!(
            !desc.contains("framerate="),
            "pipeline must never pin a framerate: {desc}"
        );
    }

    #[test]
    fn build_pipeline_desc_forces_annex_b_baseline_h264() {
        init_gst();
        // Regression guard: the Android client's MediaCodec decoder only understands Annex-B,
        // baseline-profile H.264 - see the long comment on `build_pipeline_desc` for why this
        // silently breaks decoding (no error either side) if it regresses.
        let pipeline = ScreencastPipeline::new(StreamConfig::default());
        let desc = pipeline.build_pipeline_desc(None, None);
        assert!(
            desc.contains("profile=baseline"),
            "missing baseline profile: {desc}"
        );
        assert!(
            desc.contains("stream-format=byte-stream"),
            "missing Annex-B stream format: {desc}"
        );
    }

    #[test]
    fn build_pipeline_desc_matches_configured_resolution() {
        init_gst();
        let config = StreamConfig {
            width: 2560,
            height: 1600,
            ..StreamConfig::default()
        };
        let pipeline = ScreencastPipeline::new(config);
        let desc = pipeline.build_pipeline_desc(None, None);
        assert!(desc.contains("width=2560"));
        assert!(desc.contains("height=1600"));
    }
}
