use ashpd::desktop::ResponseError;

/// Unified error type for daemon-side failures: portal/D-Bus session setup, GStreamer pipeline
/// construction, and ADB/socket I/O. Replaces ad-hoc `Result<_, String>` so callers can match on
/// specific failure kinds (e.g. a user-cancelled portal request) instead of matching on
/// human-readable message text.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error(transparent)]
    Portal(#[from] ashpd::Error),
    #[error(transparent)]
    DBus(#[from] zbus::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

impl DaemonError {
    /// Wraps a plain message, for failures that don't originate from one of the typed variants.
    pub fn other(msg: impl Into<String>) -> Self {
        DaemonError::Other(msg.into())
    }

    /// Whether this failure was the user dismissing/cancelling a portal request (e.g. the
    /// screencast source picker), as opposed to a real error worth surfacing loudly.
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self,
            DaemonError::Portal(ashpd::Error::Response(ResponseError::Cancelled))
        )
    }
}

impl From<String> for DaemonError {
    fn from(s: String) -> Self {
        DaemonError::Other(s)
    }
}

impl From<&str> for DaemonError {
    fn from(s: &str) -> Self {
        DaemonError::Other(s.to_string())
    }
}
