//! Inspect an exported Android evidence envelope without an adapter or vehicle.
use std::path::Path;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let result = match (args.next(), args.next()) {
        (Some(path), None) => obdentic::mobile::report::report(Path::new(&path)),
        _ => Err("usage: obdentic-mobile-report <android-recording.jsonl>".into()),
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
