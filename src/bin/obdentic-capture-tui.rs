use obdentic::capture_tui::run_capture_tui;
use std::{env, path::Path};

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = (|| {
        let (capture, layout) = match args.as_slice() {
            [capture] => (capture.as_str(), None),
            [capture, flag, layout] if flag == "--layout" => {
                (capture.as_str(), Some(layout.as_str()))
            }
            _ => {
                return Err(
                    "usage: obdentic-capture-tui <capture.jsonl> [--layout <layout.tsv>]".into(),
                );
            }
        };
        run_capture_tui(Path::new(capture), layout.map(Path::new))
    })();
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
