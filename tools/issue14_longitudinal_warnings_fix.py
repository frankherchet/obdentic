from pathlib import Path

path = Path("src/main.rs")
text = path.read_text()
old = """    let Ea189DpfCaptureConfig {
        profile,
        cycles,
        interval,
        include_drive_context,
    } = config;
    let started = Instant::now();
"""
new = """    let profile = config.profile;
    let started = Instant::now();
"""
if text.count(old) != 1:
    raise SystemExit(f"expected one outer capture config destructure, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
