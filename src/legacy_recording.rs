//! Legacy tab-separated `record`/`replay` file format for a single closed
//! [`Transaction`](crate::read::Transaction). Superseded for new work by the
//! capture formats, but still used to record and replay one-off reads.

use std::{collections::BTreeMap, fs, path::Path};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::{
    hex,
    read::{prepare_read, read_transaction, DiagnosticTransport, ReadRequest, Transaction},
};

pub fn record(path: &Path, transaction: &Transaction) -> Result<(), String> {
    if transaction.source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(transaction.semantic)?;
    if transaction.profile != request.profile()
        || transaction.request != request.bytes()
        || transaction.unit != request.unit()
    {
        return Err("recording profile, request or unit does not match its semantic signal".into());
    }
    let value = request.value(&transaction.response)?;
    if transaction.value != value {
        return Err("recording value does not match its raw response".into());
    }
    let transport = "ble-elm327-ffe1";
    for (field, value) in [
        ("transport", transport),
        ("source", transaction.source.as_str()),
        ("profile", transaction.profile),
        ("semantic", transaction.semantic),
        ("unit", transaction.unit),
    ] {
        reject_record_control(field, value)?;
    }
    let contents = format!(
        "OBDENTIC\t1\nprofile\t{}\ntransport\t{transport}\ntimestamp_ms\t{}\nsource\t{}\nsemantic\t{}\nrequest\t{}\nresponse\t{}\nvalue\t{}\nunit\t{}\n",
        transaction.profile,
        transaction.timestamp_ms,
        transaction.source,
        request.semantic(),
        hex(&transaction.request),
        hex(&transaction.response),
        value,
        request.unit(),
    );
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    if let Err(error) = std::io::Write::write_all(&mut file, contents.as_bytes())
        .and_then(|_| std::io::Write::flush(&mut file))
        .and_then(|_| file.sync_all())
    {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    drop(file);

    let result = fs::hard_link(&temporary, path).and_then(|_| fs::remove_file(&temporary));
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

pub async fn replay(path: &Path) -> Result<Transaction, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut lines = contents.lines();
    if lines.next() != Some("OBDENTIC\t1") {
        return Err("unsupported recording format".into());
    }
    let mut fields = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once('\t')
            .ok_or_else(|| "malformed recording field".to_string())?;
        if ![
            "profile",
            "transport",
            "timestamp_ms",
            "source",
            "semantic",
            "request",
            "response",
            "value",
            "unit",
        ]
        .contains(&name)
        {
            return Err(format!("recording contains unsupported field {name}"));
        }
        reject_record_control("field name", name)?;
        reject_record_control(name, value)?;
        if fields.insert(name, value).is_some() {
            return Err(format!("recording contains duplicate {name}"));
        }
    }
    if required(&fields, "transport")? != "ble-elm327-ffe1" {
        return Err("recording transport is unsupported".into());
    }
    let source = required(&fields, "source")?;
    if source != "user" {
        return Err("recording source must be user".into());
    }
    let request = prepare_read(required(&fields, "semantic")?)?;
    if required(&fields, "profile")? != request.profile()
        || required(&fields, "request")? != hex(&request.bytes())
        || required(&fields, "unit")? != request.unit()
    {
        return Err("recording profile, request or unit does not match its semantic signal".into());
    }
    let response = parse_hex(required(&fields, "response")?)?;
    let mut transport = ReplayTransport {
        response: Some(response),
    };
    let mut transaction = read_transaction(&mut transport, request).await?;
    let stored_value = required(&fields, "value")?
        .parse::<f64>()
        .map_err(|error| error.to_string())?;
    if stored_value != transaction.value {
        return Err("recording value does not match its raw response".into());
    }
    transaction.timestamp_ms = required(&fields, "timestamp_ms")?
        .parse::<u128>()
        .map_err(|error| error.to_string())?;
    Ok(transaction)
}

struct ReplayTransport {
    response: Option<Vec<u8>>,
}

impl DiagnosticTransport for ReplayTransport {
    async fn read(&mut self, _request: ReadRequest) -> Result<Vec<u8>, String> {
        self.response
            .take()
            .ok_or_else(|| "recording has no remaining response".into())
    }
}

fn reject_record_control(field: &str, value: &str) -> Result<(), String> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\t' | b'\r' | b'\n'))
    {
        return Err(format!("recording {field} contains a tab or newline"));
    }
    Ok(())
}

fn required<'a>(fields: &'a BTreeMap<&str, &str>, name: &str) -> Result<&'a str, String> {
    fields
        .get(name)
        .copied()
        .ok_or_else(|| format!("recording is missing {name}"))
}

fn parse_hex(value: &str) -> Result<Vec<u8>, String> {
    value
        .split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).map_err(|error| error.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "obdentic-{label}-{}-{nonce}.tsv",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn replay_recomputes_each_signal_from_raw_bytes() {
        for (semantic, request, response, value, unit) in [
            ("engine.rpm", "01 0C", "41 0C 00 00", 0.0, "rpm"),
            (
                "engine.coolant_temperature",
                "01 05",
                "41 05 5A",
                50.0,
                "°C",
            ),
            ("vehicle.speed", "01 0D", "41 0D 64", 100.0, "km/h"),
            ("engine.maf", "01 10", "41 10 01 F4", 5.0, "g/s"),
        ] {
            let path = temp_path(semantic);
            fs::write(
                &path,
                format!(
                    "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\t{semantic}\nrequest\t{request}\nresponse\t{response}\nvalue\t{value}\nunit\t{unit}\n"
                ),
            )
            .unwrap();
            let replayed = replay(&path).await.unwrap();
            assert_eq!(replayed.timestamp_ms, 1);
            assert_eq!(replayed.value, value);
            fs::remove_file(path).unwrap();
        }

        let path = temp_path("inconsistent");
        fs::write(
            &path,
            "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\tengine.maf\nrequest\t01 0C\nresponse\t41 10 01 F4\nvalue\t5\nunit\trpm\n",
        )
        .unwrap();
        assert!(replay(&path).await.is_err());
        fs::remove_file(path).unwrap();

        let path = temp_path("wrong-value");
        fs::write(
            &path,
            "OBDENTIC\t1\nprofile\tobd2-v1\ntransport\tble-elm327-ffe1\ntimestamp_ms\t1\nsource\tuser\nsemantic\tengine.rpm\nrequest\t01 0C\nresponse\t41 0C 00 00\nvalue\t999\nunit\trpm\n",
        )
        .unwrap();
        assert!(replay(&path).await.is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn record_is_private_and_does_not_overwrite_or_leave_temp_on_failure() {
        let path = temp_path("existing");
        let temporary = path.with_extension("tmp");
        fs::write(&path, "keep this recording").unwrap();
        let mut transaction = Transaction {
            timestamp_ms: 1,
            source: "user".into(),
            profile: "obd2-v1",
            semantic: "engine.rpm",
            request: vec![0x01, 0x0c],
            response: vec![0x41, 0x0c, 0x00, 0x00],
            value: 0.0,
            unit: "rpm",
        };

        assert!(record(&path, &transaction).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep this recording");
        assert!(!temporary.exists());

        #[cfg(unix)]
        {
            fs::remove_file(&path).unwrap();
            record(&path, &transaction).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::remove_file(path).unwrap();
        }

        transaction.value = 1.0;
        let path = temp_path("wrong-value");
        assert!(record(&path, &transaction).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn record_rejects_tabs_and_newlines_in_text_fields() {
        let path = temp_path("invalid");
        let transaction = Transaction {
            timestamp_ms: 1,
            source: "user\tspoof".into(),
            profile: "obd2-v1",
            semantic: "engine.rpm",
            request: vec![0x01, 0x0c],
            response: vec![0x41, 0x0c, 0x00, 0x00],
            value: 0.0,
            unit: "rpm",
        };

        assert!(record(&path, &transaction).is_err());
        assert!(!path.exists());
        assert!(!path.with_extension("tmp").exists());
    }
}
