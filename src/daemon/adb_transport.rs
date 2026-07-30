use crate::daemon::DaemonError;
use std::process::Command;
use tracing;

/// Manages Android Debug Bridge (ADB) device detection, reverse port forwarding, and system property queries.
pub struct AdbTransportManager;

impl AdbTransportManager {
    /// Locates the `adb` binary path on the host system, checking environment variables and default Android SDK locations.
    pub fn find_adb_path() -> String {
        if let Ok(path) = std::env::var("ADB_PATH") {
            return path;
        }

        let home = std::env::var("HOME").unwrap_or_default();
        let default_android_sdk_adb = format!("{}/Android/Sdk/platform-tools/adb", home);

        if std::path::Path::new(&default_android_sdk_adb).exists() {
            return default_android_sdk_adb;
        }

        "adb".to_string()
    }

    /// Lists all connected Android device serial numbers via `adb devices`.
    pub fn list_devices() -> Vec<String> {
        let adb_bin = Self::find_adb_path();
        let output = Command::new(adb_bin).arg("devices").output();

        let mut devices = Vec::new();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 && parts[1] == "device" {
                    devices.push(parts[0].to_string());
                }
            }
        }
        devices
    }

    /// Configures ADB reverse TCP port forwarding (`adb reverse tcp:local_port tcp:remote_port`) for low-latency USB streaming.
    pub fn setup_port_forwarding(
        device_serial: Option<&str>,
        local_port: u16,
        remote_port: u16,
    ) -> Result<(), DaemonError> {
        let adb_bin = Self::find_adb_path();
        tracing::info!("Using ADB binary: {}", adb_bin);
        tracing::info!(
            "Setting up ADB reverse port forward: tcp:{} -> tcp:{} (device: {:?})",
            local_port,
            remote_port,
            device_serial.unwrap_or("default")
        );

        let mut cmd = Command::new(&adb_bin);
        if let Some(serial) = device_serial {
            cmd.args(["-s", serial]);
        }
        cmd.args([
            "reverse",
            &format!("tcp:{}", remote_port),
            &format!("tcp:{}", local_port),
        ]);

        let reverse_output = cmd.output();

        match reverse_output {
            Ok(out) => {
                if out.status.success() {
                    tracing::info!("SUCCESS: ADB reverse port forward configured");
                    Ok(())
                } else {
                    let err = String::from_utf8_lossy(&out.stderr);
                    tracing::error!("ADB reverse error: {}", err);
                    Err(DaemonError::other(format!("ADB reverse failed: {}", err)))
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute adb binary ({}): {}", adb_bin, e);
                Err(DaemonError::from(e))
            }
        }
    }

    /// Queries device metadata (`ro.product.marketname`, `ro.product.model`) and determines display form factor (tablet vs phone).
    pub fn get_device_details(serial: Option<&str>) -> (String, bool) {
        let adb_bin = Self::find_adb_path();

        let query_prop = |prop: &str| -> String {
            let mut cmd = Command::new(&adb_bin);
            if let Some(s) = serial {
                cmd.args(["-s", s]);
            }
            cmd.args(["shell", "getprop", prop]);
            if let Ok(out) = cmd.output() {
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            } else {
                String::new()
            }
        };

        let market_name = query_prop("ro.product.marketname");
        let model = query_prop("ro.product.model");
        let characteristics = query_prop("ro.build.characteristics").to_lowercase();

        let display_name = if !market_name.is_empty() {
            market_name
        } else if !model.is_empty() {
            model
        } else {
            serial.unwrap_or("Android Device").to_string()
        };

        let name_lower = display_name.to_lowercase();

        let is_tablet = if characteristics.contains("tablet")
            || name_lower.contains("pad")
            || name_lower.contains("tab")
        {
            true
        } else if name_lower.contains("phone")
            || name_lower.contains("pixel")
            || name_lower.contains("galaxy s")
        {
            false
        } else {
            // Fallback: check screen aspect ratio via `wm size`
            let mut cmd_size = Command::new(&adb_bin);
            if let Some(s) = serial {
                cmd_size.args(["-s", s]);
            }
            cmd_size.args(["shell", "wm", "size"]);
            if let Ok(out) = cmd_size.output() {
                let size_str = String::from_utf8_lossy(&out.stdout);
                if let Some(dims) = size_str.split(':').nth(1) {
                    let parts: Vec<&str> = dims.trim().split('x').collect();
                    if parts.len() == 2 {
                        let w: f64 = parts[0].parse().unwrap_or(0.0);
                        let h: f64 = parts[1].parse().unwrap_or(0.0);
                        if w > 0.0 && h > 0.0 {
                            let ratio = (w.max(h)) / (w.min(h));
                            // Smartphones typically have tall aspect ratios > 1.8 (e.g. 2.16)
                            return (display_name, ratio < 1.8);
                        }
                    }
                }
            }
            true
        };

        (display_name, is_tablet)
    }
}
