from pathlib import Path

path = Path("src/vehicle_cache.rs")
text = path.read_text()
old = '        validate_text("identification knowledge revision", observation.knowledge_revision())?;'
new = '        validate_knowledge_revision(observation.knowledge_revision())?;'
if text.count(old) != 1:
    raise SystemExit("vehicle cache revision validation anchor changed unexpectedly")
text = text.replace(old, new, 1)
anchor = '\nfn validate_text(field: &str, value: &str) -> Result<(), String> {'
insert = '''
fn validate_knowledge_revision(value: &str) -> Result<(), String> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("vehicle cache knowledge revision is not a full Git object id".into());
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), String> {'''
if text.count(anchor) != 1:
    raise SystemExit("vehicle cache validate_text anchor changed unexpectedly")
path.write_text(text.replace(anchor, "\n" + insert, 1))
