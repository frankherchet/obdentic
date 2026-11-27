//! Inspect an exported Android evidence envelope without an adapter or vehicle.
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    let result = if first.as_deref() == Some(std::ffi::OsStr::new("--agent-audit")) {
        match (args.next(), args.next()) {
            (Some(path), None) => obdentic::agent_audit::inspect(&PathBuf::from(path)),
            _ => Err("usage: obdentic-mobile-report --agent-audit <audit.jsonl>".into()),
        }
    } else {
        let paths: Vec<PathBuf> = first.into_iter().chain(args).map(PathBuf::from).collect();
        if paths.is_empty() {
            Err("usage: obdentic-mobile-report <android-recording.jsonl>...".into())
        } else {
            obdentic::mobile::report::report_set(&paths)
        }
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
