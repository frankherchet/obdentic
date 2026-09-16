use crate::{capture_events::CaptureEvent, jsonl_capture, mf4_capture};
use std::path::Path;
use tokio::{sync::mpsc, task::JoinHandle};

pub type Sender = mpsc::Sender<CaptureEvent>;
pub type Writer = JoinHandle<Result<(), String>>;

/// Passive capture-writer plugin boundary.
///
/// Plugins only consume already-produced [`CaptureEvent`] values. They do not
/// receive a diagnostic session, adapter handle, scheduler, or any capability
/// that could issue vehicle traffic.
pub trait CaptureWriterPlugin: Sync {
    fn name(&self) -> &'static str;
    fn extension(&self) -> &'static str;
    fn start(&self, path: &Path) -> Result<(Sender, Writer), String>;
}

struct JsonlWriterPlugin;
struct Mf4WriterPlugin;

impl CaptureWriterPlugin for JsonlWriterPlugin {
    fn name(&self) -> &'static str {
        "jsonl"
    }

    fn extension(&self) -> &'static str {
        "jsonl"
    }

    fn start(&self, path: &Path) -> Result<(Sender, Writer), String> {
        jsonl_capture::start_jsonl(path)
    }
}

impl CaptureWriterPlugin for Mf4WriterPlugin {
    fn name(&self) -> &'static str {
        "mf4"
    }

    fn extension(&self) -> &'static str {
        "mf4"
    }

    fn start(&self, path: &Path) -> Result<(Sender, Writer), String> {
        mf4_capture::start(path)
    }
}

static JSONL_PLUGIN: JsonlWriterPlugin = JsonlWriterPlugin;
static MF4_PLUGIN: Mf4WriterPlugin = Mf4WriterPlugin;
static PLUGINS: [&dyn CaptureWriterPlugin; 2] = [&JSONL_PLUGIN, &MF4_PLUGIN];

pub fn start(path: &Path) -> Result<(Sender, Writer), String> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "capture path must end in .jsonl or .mf4".to_string())?;
    let plugin = PLUGINS
        .iter()
        .copied()
        .find(|plugin| plugin.extension().eq_ignore_ascii_case(extension))
        .ok_or_else(|| {
            format!("unsupported capture format .{extension}; expected .jsonl or .mf4")
        })?;
    plugin.start(path).map_err(|error| {
        format!(
            "{} capture writer failed for {}: {error}",
            plugin.name(),
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_capture_extension_before_opening_a_file() {
        let error = start(Path::new("capture.bin")).unwrap_err();
        assert!(error.contains("unsupported capture format .bin"));
    }
}
