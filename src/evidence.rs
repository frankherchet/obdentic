use crate::{hex, Transaction};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};
use tokio::{sync::mpsc, task::JoinHandle};

const HEADER: &str = "OBDENTIC-EVIDENCE\t1\n";
const CHANNEL_CAPACITY: usize = 64;

pub type Writer = JoinHandle<Result<(), String>>;

#[derive(Debug, PartialEq)]
pub enum EvidenceEvent {
    SessionStart {
        unix_timestamp_ms: u128,
        subscriptions: Vec<(&'static str, u64)>,
    },
    Read {
        semantic: &'static str,
        requested_interval_ms: u64,
        scheduled_offset_ms: u128,
        read_started_offset_ms: u128,
        read_finished_offset_ms: u128,
        read_duration_ms: u128,
        transaction: Transaction,
    },
    ReadError {
        semantic: &'static str,
        requested_interval_ms: u64,
        scheduled_offset_ms: u128,
        read_started_offset_ms: u128,
        read_finished_offset_ms: u128,
        read_duration_ms: u128,
        error: String,
    },
    SessionStop {
        offset_ms: u128,
    },
}

pub fn start(path: &Path) -> Result<(mpsc::Sender<EvidenceEvent>, Writer), String> {
    let mut options = OpenOptions::new();
    options.append(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(HEADER.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }

    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let task = tokio::task::spawn_blocking(move || write_events(file, receiver));
    Ok((sender, task))
}

fn write_events(mut file: File, mut receiver: mpsc::Receiver<EvidenceEvent>) -> Result<(), String> {
    let result = (|| {
        while let Some(event) = receiver.blocking_recv() {
            write_event(&mut file, event)?;
        }
        Ok::<(), String>(())
    })();

    let close = file
        .flush()
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string());
    match (result, close) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(close)) => Err(format!("{error}; close failed: {close}")),
    }
}

fn write_event(file: &mut File, event: EvidenceEvent) -> Result<(), String> {
    let line = match event {
        EvidenceEvent::SessionStart {
            unix_timestamp_ms,
            subscriptions,
        } => {
            let mut line = format!(
                "session_start\t{unix_timestamp_ms}\t{}",
                subscriptions.len()
            );
            for (semantic, interval_ms) in subscriptions {
                line.push('\t');
                line.push_str(&escape_field(semantic));
                line.push('\t');
                line.push_str(&interval_ms.to_string());
            }
            line.push('\n');
            line
        }
        EvidenceEvent::Read {
            semantic,
            requested_interval_ms,
            scheduled_offset_ms,
            read_started_offset_ms,
            read_finished_offset_ms,
            read_duration_ms,
            transaction,
        } => format!(
            "read\t{}\t{requested_interval_ms}\t{scheduled_offset_ms}\t{read_started_offset_ms}\t{read_finished_offset_ms}\t{read_duration_ms}\tsuccess\t{}\t{}\t{}\t{}\t\n",
            escape_field(semantic),
            hex(transaction.request()),
            hex(transaction.response()),
            transaction.value(),
            escape_field(transaction.unit()),
        ),
        EvidenceEvent::ReadError {
            semantic,
            requested_interval_ms,
            scheduled_offset_ms,
            read_started_offset_ms,
            read_finished_offset_ms,
            read_duration_ms,
            error,
        } => format!(
            "read\t{}\t{requested_interval_ms}\t{scheduled_offset_ms}\t{read_started_offset_ms}\t{read_finished_offset_ms}\t{read_duration_ms}\terror\t\t\t\t\t{}\n",
            escape_field(semantic),
            escape_field(&error),
        ),
        EvidenceEvent::SessionStop { offset_ms } => format!("session_stop\t{offset_ms}\n"),
    };
    file.write_all(line.as_bytes())
        .map_err(|error| error.to_string())
}

fn escape_field(value: &str) -> String {
    value.escape_default().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare_read;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-evidence-{label}-{}-{nonce}.tsv",
            std::process::id()
        ))
    }

    async fn close(sender: mpsc::Sender<EvidenceEvent>, task: Writer) {
        drop(sender);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn writes_header_and_versioned_event_lines() {
        let path = temp_path("events");
        let (sender, task) = start(&path).unwrap();
        let transaction = prepare_read("engine.rpm")
            .unwrap()
            .complete("u\tser\n", vec![0x41, 0x0c, 0x00, 0x00])
            .unwrap()
            .with_timestamp_ms(11);
        sender
            .send(EvidenceEvent::SessionStart {
                unix_timestamp_ms: 1_700_000_000_000,
                subscriptions: vec![("engine.rpm", 200)],
            })
            .await
            .unwrap();
        sender
            .send(EvidenceEvent::Read {
                semantic: "engine.rpm",
                requested_interval_ms: 200,
                scheduled_offset_ms: 0,
                read_started_offset_ms: 2,
                read_finished_offset_ms: 11,
                read_duration_ms: 9,
                transaction,
            })
            .await
            .unwrap();
        sender
            .send(EvidenceEvent::ReadError {
                semantic: "engine.rpm",
                requested_interval_ms: 200,
                scheduled_offset_ms: 200,
                read_started_offset_ms: 201,
                read_finished_offset_ms: 202,
                read_duration_ms: 1,
                error: "timeout\tline\n".into(),
            })
            .await
            .unwrap();
        sender
            .send(EvidenceEvent::SessionStop { offset_ms: 300 })
            .await
            .unwrap();
        close(sender, task).await;

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with(HEADER));
        assert!(contents.contains("session_start\t1700000000000\t1\tengine.rpm\t200\n"));
        assert!(contents.contains(
            "read\tengine.rpm\t200\t0\t2\t11\t9\tsuccess\t01 0C\t41 0C 00 00\t0\trpm\t\n"
        ));
        assert!(contents.contains(
            "read\tengine.rpm\t200\t200\t201\t202\t1\terror\t\t\t\t\ttimeout\\tline\\n\n"
        ));
        assert!(contents.ends_with("session_stop\t300\n"));
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn creates_private_file_and_refuses_overwrite() {
        let path = temp_path("private");
        let (sender, task) = start(&path).unwrap();
        close(sender, task).await;

        assert!(start(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_file(path).unwrap();
    }
}
