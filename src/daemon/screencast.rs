use crate::daemon::StreamConfig;
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

    /// Auto-detects and returns configured H.264 encoder string with zero-latency parameters.
    fn detect_encoder_pipeline_string() -> String {
        if gstreamer::ElementFactory::find("nvh264enc").is_some() {
            tracing::info!("Selected NVIDIA NVENC Hardware Encoder (nvh264enc) [Zero Latency]");
            "nvh264enc zerolatency=true tune=ultra-low-latency rc-mode=cbr bitrate=12000 gop-size=30 bframes=0".to_string()
        } else if gstreamer::ElementFactory::find("vah264enc").is_some() {
            tracing::info!("Selected VA-API Hardware Encoder (vah264enc)");
            "vah264enc rate-control=cbr bitrate=12000 gop-size=30 bframes=0".to_string()
        } else if gstreamer::ElementFactory::find("vaapih264enc").is_some() {
            tracing::info!("Selected VA-API Hardware Encoder (vaapih264enc)");
            "vaapih264enc rate-control=cbr bitrate=12000 keyframe-period=30".to_string()
        } else if gstreamer::ElementFactory::find("x264enc").is_some() {
            tracing::info!("Selected x264 Software Encoder (x264enc) [Zero Latency]");
            "x264enc tune=zerolatency speed-preset=ultrafast bitrate=12000 key-int-max=30 bframes=0 threads=4 sync-lookahead=0 rc-lookahead=0".to_string()
        } else {
            tracing::info!("Selected OpenH264 Software Encoder (openh264enc)");
            "openh264enc gop-size=30 bitrate=12000000".to_string()
        }
    }

    /// Constructs GStreamer pipeline string for PipeWire src -> H.264 enc -> AppSink.
    pub fn build_pipeline_desc(&self, node_id: Option<u32>, fd: Option<i32>) -> String {
        let encoder_str = Self::detect_encoder_pipeline_string();
        let src = match (fd, node_id) {
            (Some(fd_val), Some(id)) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time=16 fd={} path={}",
                fd_val, id
            ),
            (Some(fd_val), None) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time=16 fd={}",
                fd_val
            ),
            (None, Some(id)) => format!(
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time=16 path={}",
                id
            ),
            (None, None) => {
                "pipewiresrc do-timestamp=true always-copy=true keepalive-time=16".to_string()
            }
        };
        let caps = format!(
            "video/x-raw,width={},height={}",
            self.config.width, self.config.height
        );
        format!(
            "{} ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! videoconvert n-threads=4 ! videoscale ! {} ! queue max-size-buffers=1 max-size-bytes=0 max-size-time=0 leaky=downstream ! {} ! h264parse config-interval=1 ! appsink name=sink sync=false max-buffers=1 drop=true",
            src, caps, encoder_str
        )
    }

    /// Launches the GStreamer pipeline and binds local TCP socket listener for video stream delivery.
    pub fn start(
        &mut self,
        port: u16,
        node_id: Option<u32>,
        fd: Option<i32>,
    ) -> Result<(), String> {
        gstreamer::init().map_err(|e| format!("Failed to init GStreamer: {}", e))?;

        let pipeline_str = self.build_pipeline_desc(node_id, fd);
        tracing::info!("Launching GStreamer Screencast Pipeline: {}", pipeline_str);

        let element = gstreamer::parse::launch(&pipeline_str)
            .map_err(|e| format!("Failed to parse pipeline: {}", e))?;

        let pipeline = element
            .dynamic_cast::<gstreamer::Pipeline>()
            .map_err(|_| "Element is not a Pipeline".to_string())?;

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| "Failed to find appsink element".to_string())?
            .dynamic_cast::<AppSink>()
            .map_err(|_| "Element 'sink' is not an AppSink".to_string())?;

        let active_stream: Arc<Mutex<Option<std::net::TcpStream>>> = Arc::new(Mutex::new(None));
        let active_stream_listener = active_stream.clone();

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_signal_thread = stop_signal.clone();

        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .map_err(|e| format!("Failed to bind TCP listener on port {}: {}", port, e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking on TCP listener: {}", e))?;

        thread::spawn(move || {
            while !stop_signal_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        tracing::info!("Video stream client connected from {}", addr);
                        let _ = stream.set_nodelay(true);
                        let mut guard = active_stream_listener.lock().unwrap();
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
                        return Err(err_msg);
                    }
                }
            }
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Err(format!("Failed to set pipeline to Playing: {}", e));
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
