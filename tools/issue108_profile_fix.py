from pathlib import Path

path = Path("src/capture.rs")
text = path.read_text()
old = "if observation.semantic.contains(['*', '?']) {"
new = "if observation.semantic.contains('*') || observation.semantic.contains('?') {"
if text.count(old) != 1:
    raise SystemExit(f"expected one wildcard check, found {text.count(old)}")
path.write_text(text.replace(old, new, 1))
