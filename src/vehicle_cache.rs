//! A small, local-only cache of already collected vehicle evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const HEADER: &str = "OBDENTIC-VEHICLE-CACHE\t1";
const INDEX_HEADER: &str = "OBDENTIC-VEHICLE-INDEX\t1";
const INDEX_NAME: &str = ".identity-index";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VehicleCache {
    local_key: String,
    first_seen_ms: u64,
    last_seen_ms: u64,
    evidence: Vec<String>,
}

impl VehicleCache {
    pub fn new(
        local_key: impl Into<String>,
        first_seen_ms: u64,
        last_seen_ms: u64,
        mut evidence: Vec<String>,
    ) -> Self {
        evidence.sort_unstable();
        Self {
            local_key: local_key.into(),
            first_seen_ms,
            last_seen_ms,
            evidence,
        }
    }

    pub fn local_key(&self) -> &str {
        &self.local_key
    }

    pub const fn first_seen_ms(&self) -> u64 {
        self.first_seen_ms
    }

    pub const fn last_seen_ms(&self) -> u64 {
        self.last_seen_ms
    }

    pub fn evidence(&self) -> &[String] {
        &self.evidence
    }
}

pub struct CacheStore {
    root: PathBuf,
}

impl CacheStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_owned(),
        }
    }

    pub fn save(&self, cache: &VehicleCache) -> Result<(), String> {
        validate_cache(cache)?;
        fs::create_dir_all(&self.root).map_err(|error| error.to_string())?;

        let path = self.path_for(&cache.local_key);
        let temporary = path.with_extension("tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;

        let result = (|| {
            file.write_all(HEADER.as_bytes())?;
            file.write_all(b"\nlocal_key\t")?;
            file.write_all(escape(&cache.local_key).as_bytes())?;
            file.write_all(b"\nfirst_seen_ms\t")?;
            file.write_all(cache.first_seen_ms.to_string().as_bytes())?;
            file.write_all(b"\nlast_seen_ms\t")?;
            file.write_all(cache.last_seen_ms.to_string().as_bytes())?;
            for line in &cache.evidence {
                file.write_all(b"\nevidence\t")?;
                file.write_all(escape(line).as_bytes())?;
            }
            file.write_all(b"\n")?;
            file.flush()?;
            file.sync_all()
        })();
        if let Err(error) = result {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        drop(file);

        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn load(&self, local_key: &str) -> Result<Option<VehicleCache>, String> {
        validate_text("local key", local_key)?;
        let path = self.path_for(local_key);
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        parse(&contents, local_key).map(Some)
    }

    pub fn load_all(&self) -> Result<Vec<VehicleCache>, String> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };
        let mut paths = entries
            .map(|entry| entry.map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|entry| {
                (entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("tsv"))
                .then_some(entry.path())
            })
            .collect::<Vec<_>>();
        paths.sort();

        paths
            .into_iter()
            .map(|path| {
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| "vehicle cache filename is not valid UTF-8".to_string())?;
                let local_key = decode_hex(stem)?;
                self.load(&local_key)?.ok_or_else(|| {
                    format!("vehicle cache disappeared while loading {}", path.display())
                })
            })
            .collect()
    }

    fn path_for(&self, local_key: &str) -> PathBuf {
        self.root.join(format!("{}.tsv", hex(local_key.as_bytes())))
    }
}

/// Private VIN-to-key correlation kept separate from vehicle cache records.
/// The VIN is intentionally present only in this 0600 index file.
pub struct VehicleIndex {
    path: PathBuf,
}

impl VehicleIndex {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            path: root.as_ref().join(INDEX_NAME),
        }
    }

    pub fn key_for(&self, vin: &crate::Vin) -> Result<Option<String>, String> {
        Ok(self.read()?.get(vin.as_str()).cloned())
    }

    pub fn key_for_or_create(&self, vin: &crate::Vin) -> Result<(String, bool), String> {
        let mut entries = self.read()?;
        if let Some(key) = entries.get(vin.as_str()) {
            return Ok((key.clone(), false));
        }

        let key = random_key()?;
        entries.insert(vin.as_str().into(), key.clone());
        self.write(&entries)?;
        Ok((key, true))
    }

    fn read(&self) -> Result<BTreeMap<String, String>, String> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new())
            }
            Err(error) => return Err(error.to_string()),
        };
        parse_index(&contents)
    }

    fn write(&self, entries: &BTreeMap<String, String>) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = self.path.with_extension("tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            writeln!(file, "{INDEX_HEADER}")?;
            for (vin, key) in entries {
                writeln!(file, "{}\t{}", escape(vin), escape(key))?;
            }
            file.flush()?;
            file.sync_all()
        })();
        if let Err(error) = result {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
}

fn parse_index(contents: &str) -> Result<BTreeMap<String, String>, String> {
    let mut lines = contents.lines();
    if lines.next() != Some(INDEX_HEADER) {
        return Err("unsupported vehicle index format".into());
    }
    let mut entries = BTreeMap::new();
    let mut keys = BTreeSet::new();
    for line in lines {
        let (encoded_vin, encoded_key) = line
            .split_once('\t')
            .ok_or_else(|| "malformed vehicle index field".to_string())?;
        let vin = unescape(encoded_vin)?;
        crate::Vin::parse(&vin).map_err(|error| format!("invalid vehicle index VIN: {error}"))?;
        let key = unescape(encoded_key)?;
        validate_text("local key", &key)?;
        if !keys.insert(key.clone()) {
            return Err("vehicle index maps multiple VINs to one local key".into());
        }
        if entries.insert(vin, key).is_some() {
            return Err("vehicle index contains duplicate VIN".into());
        }
    }
    Ok(entries)
}

fn decode_hex(value: &str) -> Result<String, String> {
    if value.is_empty() || !value.len().is_multiple_of(2) {
        return Err("vehicle cache filename is not hexadecimal".into());
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let pair = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
        bytes.push(
            u8::from_str_radix(pair, 16)
                .map_err(|_| "vehicle cache filename is not hexadecimal".to_string())?,
        );
    }
    String::from_utf8(bytes).map_err(|_| "vehicle cache filename is not UTF-8".into())
}

fn random_key() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    fs::File::open("/dev/urandom")
        .map_err(|error| format!("unable to generate vehicle key: {error}"))?
        .read_exact(&mut bytes)
        .map_err(|error| format!("unable to generate vehicle key: {error}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ))
}

fn parse(contents: &str, requested_key: &str) -> Result<VehicleCache, String> {
    let mut lines = contents.lines();
    if lines.next() != Some(HEADER) {
        return Err("unsupported vehicle cache format".into());
    }

    let mut local_key = None;
    let mut first_seen_ms = None;
    let mut last_seen_ms = None;
    let mut evidence = Vec::new();
    for line in lines {
        let (field, encoded) = line
            .split_once('\t')
            .ok_or_else(|| "malformed vehicle cache field".to_string())?;
        match field {
            "local_key" => {
                if local_key.is_some() {
                    return Err("vehicle cache contains duplicate local_key".into());
                }
                local_key = Some(unescape(encoded)?);
            }
            "first_seen_ms" => {
                if first_seen_ms.is_some() {
                    return Err("vehicle cache contains duplicate first_seen_ms".into());
                }
                first_seen_ms = Some(
                    encoded
                        .parse::<u64>()
                        .map_err(|error| format!("invalid first_seen_ms: {error}"))?,
                );
            }
            "last_seen_ms" => {
                if last_seen_ms.is_some() {
                    return Err("vehicle cache contains duplicate last_seen_ms".into());
                }
                last_seen_ms = Some(
                    encoded
                        .parse::<u64>()
                        .map_err(|error| format!("invalid last_seen_ms: {error}"))?,
                );
            }
            "evidence" => evidence.push(unescape(encoded)?),
            _ => return Err(format!("vehicle cache contains unsupported field {field}")),
        }
    }

    let cache = VehicleCache::new(
        local_key.ok_or_else(|| "vehicle cache is missing local_key".to_string())?,
        first_seen_ms.ok_or_else(|| "vehicle cache is missing first_seen_ms".to_string())?,
        last_seen_ms.ok_or_else(|| "vehicle cache is missing last_seen_ms".to_string())?,
        evidence,
    );
    validate_cache(&cache)?;
    if cache.local_key != requested_key {
        return Err("vehicle cache local_key does not match its path".into());
    }
    Ok(cache)
}

fn validate_cache(cache: &VehicleCache) -> Result<(), String> {
    validate_text("local key", &cache.local_key)?;
    if cache.last_seen_ms < cache.first_seen_ms {
        return Err("vehicle cache last_seen_ms precedes first_seen_ms".into());
    }
    for line in &cache.evidence {
        validate_text("evidence", line)?;
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains_raw_vin() {
        return Err(format!(
            "vehicle cache {field} is empty or contains a raw VIN"
        ));
    }
    Ok(())
}

trait RawVin {
    fn contains_raw_vin(&self) -> bool;
}

impl RawVin for str {
    fn contains_raw_vin(&self) -> bool {
        self.as_bytes().windows(17).any(|window| {
            window.iter().all(|byte| {
                matches!(byte.to_ascii_uppercase(), b'A'..=b'H' | b'J'..=b'N' | b'P'..=b'R' | b'S'..=b'Z' | b'0'..=b'9')
            })
        })
    }
}

fn escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            '\n' => escaped.push_str("\\n"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn unescape(value: &str) -> Result<String, String> {
    let mut unescaped = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        unescaped.push(match characters.next() {
            Some('\\') => '\\',
            Some('t') => '\t',
            Some('r') => '\r',
            Some('n') => '\n',
            _ => return Err("vehicle cache contains an invalid escape".into()),
        });
    }
    Ok(unescaped)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("obdentic-vehicle-cache-{label}-{nonce}"))
    }

    #[test]
    fn round_trips_sorted_evidence() {
        let root = root("roundtrip");
        let store = CacheStore::new(&root);
        let cache = VehicleCache::new("local-key", 10, 20, vec!["zeta".into(), "alpha".into()]);
        store.save(&cache).unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert_eq!(loaded.local_key(), "local-key");
        assert_eq!(loaded.first_seen_ms(), 10);
        assert_eq!(loaded.last_seen_ms(), 20);
        assert_eq!(loaded.evidence(), ["alpha", "zeta"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn writes_only_versioned_tsv_fields() {
        let root = root("format");
        let store = CacheStore::new(&root);
        store
            .save(&VehicleCache::new(
                "local-key",
                1,
                2,
                vec!["evidence".into()],
            ))
            .unwrap();
        let path = root.join("6c6f63616c2d6b6579.tsv");
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "OBDENTIC-VEHICLE-CACHE\t1\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t2\nevidence\tevidence\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saves_atomically_with_private_permissions() {
        let root = root("private");
        let store = CacheStore::new(&root);
        store
            .save(&VehicleCache::new("local-key", 1, 1, Vec::new()))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("6c6f63616c2d6b6579.tsv");
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(!root.join("6c6f63616c2d6b6579.tmp").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_raw_vin_and_unsupported_versions() {
        let root = root("reject");
        let store = CacheStore::new(&root);
        let vin = "WVWZZZ1JZXW000001";
        assert!(store
            .save(&VehicleCache::new(vin, 1, 1, Vec::new()))
            .is_err());
        assert!(store
            .save(&VehicleCache::new("local-key", 1, 1, vec![vin.into()]))
            .is_err());

        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("6c6f63616c2d6b6579.tsv"),
            "OBDENTIC-VEHICLE-CACHE\t2\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t1\n",
        )
        .unwrap();
        assert!(store.load("local-key").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_all_cache_records_in_filename_order() {
        let root = root("all");
        let store = CacheStore::new(&root);
        store
            .save(&VehicleCache::new("second", 2, 2, Vec::new()))
            .unwrap();
        store
            .save(&VehicleCache::new("first", 1, 1, Vec::new()))
            .unwrap();

        let caches = store.load_all().unwrap();
        assert_eq!(
            caches
                .iter()
                .map(VehicleCache::local_key)
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_index_reuses_keys_and_keeps_vin_out_of_cache_records() {
        let root = root("index");
        let store = CacheStore::new(&root);
        let index = VehicleIndex::new(&root);
        let vin = crate::Vin::parse("WVWZZZ1JZXW000001").unwrap();
        let (key, created) = index.key_for_or_create(&vin).unwrap();
        assert!(created);
        assert_eq!(index.key_for(&vin).unwrap(), Some(key.clone()));
        assert_eq!(index.key_for_or_create(&vin).unwrap(), (key.clone(), false));

        store
            .save(&VehicleCache::new(key, 1, 1, vec!["responder=7E8".into()]))
            .unwrap();
        let cache_path = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("tsv"))
            .unwrap();
        assert!(!cache_path.to_string_lossy().contains(vin.as_str()));
        assert!(!fs::read_to_string(cache_path)
            .unwrap()
            .contains(vin.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join(INDEX_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
