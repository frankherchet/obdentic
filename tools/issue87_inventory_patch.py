from pathlib import Path

path = Path("src/lib.rs")
text = path.read_text()
old = "pub mod vehicle_cache;\npub mod vehicle_knowledge;"
new = "pub mod vehicle_cache;\npub mod vehicle_inventory;\npub mod vehicle_knowledge;"
if text.count(old) != 1:
    raise SystemExit("src/lib.rs inventory module anchor changed unexpectedly")
path.write_text(text.replace(old, new, 1))
