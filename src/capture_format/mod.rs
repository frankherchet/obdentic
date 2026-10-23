//! One module for every capture file format.
//!
//! [`CaptureSink`] is the single write interface: it picks JSONL or MF4 by
//! the file extension and hides the writer registry that used to bounce
//! between `capture_writer` and `jsonl_capture`/`mf4_capture`. [`read`] is
//! the single read interface; MF4 has no reader, so every capture reader in
//! the crate depends on this module, not on the JSONL codec directly.

mod jsonl;
mod mf4;

use crate::capture_events::CaptureEvent;
use std::path::Path;
use tokio::{sync::mpsc, task::JoinHandle};

pub use jsonl::{CaptureStatus, ParsedCapture};
pub(crate) use jsonl::VERSION;

pub type CaptureSender = mpsc::Sender<CaptureEvent>;
type Writer = JoinHandle<Result<(), String>>;

enum CaptureFormat {
    Jsonl,
    Mf4,
}

impl CaptureFormat {
    fn from_path(path: &Path) -> Result<Self, String> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "capture path must end in .jsonl or .mf4".to_string())?;
        if extension.eq_ignore_ascii_case("jsonl") {
            Ok(Self::Jsonl)
        } else if extension.eq_ignore_ascii_case("mf4") {
            Ok(Self::Mf4)
        } else {
            Err(format!(
                "unsupported capture format .{extension}; expected .jsonl or .mf4"
            ))
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Mf4 => "mf4",
        }
    }

    fn open(&self, path: &Path) -> Result<(CaptureSender, Writer), String> {
        match self {
            Self::Jsonl => jsonl::start(path),
            Self::Mf4 => mf4::start(path),
        }
    }
}

/// One capture sink, chosen by the file extension. JSONL and MF4 differ
/// only in how `open` writes an event; every caller after that talks to one
/// interface regardless of format.
#[derive(Debug)]
pub struct CaptureSink {
    sender: Option<CaptureSender>,
    writer: Option<Writer>,
}

impl CaptureSink {
    /// Starts a private recorder, replacing an earlier capture at the same path.
    pub fn open(path: &Path) -> Result<Self, String> {
        let format = CaptureFormat::from_path(path)?;
        let (sender, writer) = format.open(path).map_err(|error| {
            format!(
                "{} capture writer failed for {}: {error}",
                format.name(),
                path.display()
            )
        })?;
        Ok(Self {
            sender: Some(sender),
            writer: Some(writer),
        })
    }

    pub fn sender(&self) -> &CaptureSender {
        self.sender
            .as_ref()
            .expect("capture sink sender already closed")
    }

    pub async fn close(mut self) -> Result<(), String> {
        self.sender.take();
        self.writer
            .take()
            .expect("capture sink writer already closed")
            .await
            .map_err(|error| format!("capture recorder stopped unexpectedly: {error}"))?
    }
}

/// Reads a capture back. Only JSONL is readable; MF4 is a write-only audit
/// trail (see `docs/mf4-capture.md`).
pub fn read(path: &Path) -> Result<ParsedCapture, String> {
    jsonl::read(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_events::CaptureEvent;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_unknown_capture_extension_before_opening_a_file() {
        let error = CaptureSink::open(Path::new("capture.bin")).unwrap_err();
        assert!(error.contains("unsupported capture format .bin"));
    }

    #[test]
    fn rejects_a_path_with_no_extension() {
        let error = CaptureSink::open(Path::new("capture")).unwrap_err();
        assert_eq!(error, "capture path must end in .jsonl or .mf4");
    }

    fn temp_path(extension: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-capture-sink-{}-{nonce}.{extension}",
            std::process::id()
        ))
    }

    fn sample_events() -> Vec<CaptureEvent> {
        vec![
            CaptureEvent::capture_started(Some(1_700_000_000_000), Some("test".into())),
            CaptureEvent::SessionInitialized,
        ]
    }

    #[tokio::test]
    async fn jsonl_sink_round_trips_through_the_shared_interface() {
        let path = temp_path("jsonl");
        let sink = CaptureSink::open(&path).unwrap();
        for event in sample_events() {
            sink.sender().send(event).await.unwrap();
        }
        sink.close().await.unwrap();

        let parsed = read(&path).unwrap();
        assert_eq!(parsed.events, sample_events());
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn mf4_sink_writes_a_valid_mdf_file() {
        let path = temp_path("mf4");
        let sink = CaptureSink::open(&path).unwrap();
        for event in sample_events() {
            sink.sender().send(event).await.unwrap();
        }
        sink.close().await.unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(&contents[..3], b"MDF", "MF4 files start with the MDF magic bytes");
        std::fs::remove_file(path).unwrap();
    }
}
