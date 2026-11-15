//! Inspect an exported Android evidence envelope without an adapter or vehicle.
use std::path::PathBuf;

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let result = if paths.is_empty() {
        Err("usage: obdentic-mobile-report <android-recording.jsonl>...".into())
    } else {
        obdentic::mobile::report::report_set(&paths)
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
