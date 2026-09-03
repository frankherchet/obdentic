//! A small, local-only cache of already collected vehicle evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    ecu_identification::{
        IdentificationObservation, IdentificationResponseEvidence, IdentificationResultStatus,
    },
    functional_discovery::EcuCapability,
    topology::{
        AddressingContext, Confidence, EcuRole, EcuTopology, ObservationWindow, Protocol,
        ProtocolContext, Provenance, RequestAddress, RequestTarget, RequestTargetEvidence,
        ResponderIdentity, RoleAssignment,
    },
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const HEADER: &str = "OBDENTIC-VEHICLE-CACHE\t4";
const V3_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\t3";
const V2_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\t2";
const LEGACY_HEADER: &str = "OBDENTIC-VEHICLE-CACHE\t1";
const INDEX_HEADER: &str = "OBDENTIC-VEHICLE-INDEX\t1";
const INDEX_NAME: &str = ".identity-index";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VehicleCache {
    local_key: String,
    first_seen_ms: u64,
    last_seen_ms: u64,
    snapshot: VehicleCacheSnapshot,
    history: Vec<String>,
}

impl VehicleCache {
    pub fn new(
        local_key: impl Into<String>,
        first_seen_ms: u64,
        last_seen_ms: u64,
        history: Vec<String>,
    ) -> Self {
        Self::with_snapshot(
            local_key,
            first_seen_ms,
            last_seen_ms,
            VehicleCacheSnapshot::default(),
            history,
        )
    }

    pub fn with_snapshot(
        local_key: impl Into<String>,
        first_seen_ms: u64,
        last_seen_ms: u64,
        snapshot: VehicleCacheSnapshot,
        mut history: Vec<String>,
    ) -> Self {
        history.sort_unstable();
        Self {
            local_key: local_key.into(),
            first_seen_ms,
            last_seen_ms,
            snapshot,
            history,
        }
    }

    pub fn new_with_snapshot(
        local_key: impl Into<String>,
        first_seen_ms: u64,
        last_seen_ms: u64,
        snapshot: VehicleCacheSnapshot,
        history: Vec<String>,
    ) -> Self {
        Self::with_snapshot(local_key, first_seen_ms, last_seen_ms, snapshot, history)
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
        &self.history
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub fn snapshot(&self) -> &VehicleCacheSnapshot {
        &self.snapshot
    }
}

/// The current, typed state of one vehicle.  Historical text is deliberately
/// not part of this value and therefore cannot become validation input.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct VehicleCacheSnapshot {
    topology: Vec<TopologyObservation>,
    ecu_capabilities: Vec<EcuCapabilitySnapshot>,
    target_mappings: Vec<TargetMappingSnapshot>,
    ecu_identification: Vec<IdentificationObservation>,
}

impl VehicleCacheSnapshot {
    pub fn new(
        topology: impl IntoIterator<Item = TopologyObservation>,
        ecu_capabilities: impl IntoIterator<Item = EcuCapabilitySnapshot>,
        target_mappings: impl IntoIterator<Item = TargetMappingSnapshot>,
    ) -> Self {
        Self::with_ecu_identification(topology, ecu_capabilities, target_mappings, [])
    }

    pub fn with_ecu_identification(
        topology: impl IntoIterator<Item = TopologyObservation>,
        ecu_capabilities: impl IntoIterator<Item = EcuCapabilitySnapshot>,
        target_mappings: impl IntoIterator<Item = TargetMappingSnapshot>,
        ecu_identification: impl IntoIterator<Item = IdentificationObservation>,
    ) -> Self {
        let mut snapshot = Self {
            topology: topology.into_iter().collect(),
            ecu_capabilities: ecu_capabilities.into_iter().collect(),
            target_mappings: target_mappings.into_iter().collect(),
            ecu_identification: ecu_identification.into_iter().collect(),
        };
        snapshot.topology.sort();
        snapshot.ecu_capabilities.sort();
        snapshot.target_mappings.sort();
        snapshot.ecu_identification.sort();
        snapshot
    }

    pub fn from_topology(topology: &EcuTopology) -> Self {
        let mut observations = Vec::new();
        let mut mappings = Vec::new();
        for node in topology.nodes() {
            for responder in node.observed_responders() {
                observations.push(TopologyObservation::from_responder(
                    node.context().clone(),
                    responder,
                ));
                if let Some(target) = node.request_target() {
                    mappings.push(TargetMappingSnapshot::new(
                        node.role().cloned(),
                        Some(responder.identity().clone()),
                        target.target().clone(),
                        target.provenance().clone(),
                    ));
                }
            }
            if node.observed_responders().is_empty() {
                if let Some(target) = node.request_target() {
                    mappings.push(TargetMappingSnapshot::new(
                        node.role().cloned(),
                        None,
                        target.target().clone(),
                        target.provenance().clone(),
                    ));
                }
            }
        }
        Self::new(observations, [], mappings)
    }

    pub fn from_discovery(topology: &EcuTopology, capabilities: &[EcuCapability]) -> Self {
        let mut snapshot = Self::from_topology(topology);
        snapshot.ecu_capabilities = capabilities
            .iter()
            .map(EcuCapabilitySnapshot::from_capability)
            .collect();
        snapshot.ecu_capabilities.sort();
        snapshot
    }

    pub fn topology(&self) -> &[TopologyObservation] {
        &self.topology
    }

    pub fn topology_observations(&self) -> &[TopologyObservation] {
        self.topology()
    }

    pub fn ecu_capabilities(&self) -> &[EcuCapabilitySnapshot] {
        &self.ecu_capabilities
    }

    pub fn capabilities(&self) -> &[EcuCapabilitySnapshot] {
        self.ecu_capabilities()
    }

    pub fn target_mappings(&self) -> &[TargetMappingSnapshot] {
        &self.target_mappings
    }

    pub fn ecu_identification(&self) -> &[IdentificationObservation] {
        &self.ecu_identification
    }

    pub fn validation_signature(&self) -> ValidationSignature {
        let topology = self
            .topology
            .iter()
            .filter(|observation| {
                observation
                    .payload()
                    .is_some_and(|payload| payload.starts_with(&[0x41, 0x00]))
            })
            .cloned()
            .collect();
        let ecu_capabilities = self
            .ecu_capabilities
            .iter()
            .filter_map(|capability| {
                let pages = capability
                    .pages
                    .iter()
                    .filter(|page| page.request == [0x01, 0x00])
                    .cloned()
                    .collect::<Vec<_>>();
                (!pages.is_empty())
                    .then(|| EcuCapabilitySnapshot::new(capability.responder.clone(), pages))
            })
            .collect();
        ValidationSignature {
            topology,
            ecu_capabilities,
            target_mappings: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TopologyObservation {
    context: ProtocolContext,
    responder: ResponderIdentity,
    payload: Option<Vec<u8>>,
    observation: Option<ObservationWindow>,
    provenance: Provenance,
}

impl TopologyObservation {
    pub fn new(
        context: ProtocolContext,
        responder: ResponderIdentity,
        payload: Option<Vec<u8>>,
        observation: Option<ObservationWindow>,
        provenance: Provenance,
    ) -> Self {
        Self {
            context,
            responder,
            payload,
            observation,
            provenance,
        }
    }

    fn from_responder(
        context: ProtocolContext,
        responder: &crate::topology::ObservedResponder,
    ) -> Self {
        Self::new(
            context,
            responder.identity().clone(),
            responder.payload().map(<[u8]>::to_vec),
            responder.observation(),
            responder.provenance().clone(),
        )
    }

    pub fn context(&self) -> &ProtocolContext {
        &self.context
    }

    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn payload(&self) -> Option<&[u8]> {
        self.payload.as_deref()
    }

    pub fn observation(&self) -> Option<ObservationWindow> {
        self.observation
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapabilityPageSnapshot {
    request: [u8; 2],
    payload: Vec<u8>,
    provenance: Provenance,
}

impl CapabilityPageSnapshot {
    pub fn new(request: [u8; 2], payload: Vec<u8>, provenance: Provenance) -> Self {
        Self {
            request,
            payload,
            provenance,
        }
    }

    pub fn request(&self) -> [u8; 2] {
        self.request
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EcuCapabilitySnapshot {
    responder: ResponderIdentity,
    pages: Vec<CapabilityPageSnapshot>,
}

impl EcuCapabilitySnapshot {
    pub fn new(
        responder: ResponderIdentity,
        pages: impl IntoIterator<Item = CapabilityPageSnapshot>,
    ) -> Self {
        let mut pages = pages.into_iter().collect::<Vec<_>>();
        pages.sort();
        Self { responder, pages }
    }

    fn from_capability(capability: &EcuCapability) -> Self {
        Self::new(
            capability.responder().clone(),
            capability.mode01_pages().iter().map(|page| {
                CapabilityPageSnapshot::new(
                    page.request(),
                    page.payload().to_vec(),
                    page.provenance().clone(),
                )
            }),
        )
    }

    pub fn responder(&self) -> &ResponderIdentity {
        &self.responder
    }

    pub fn pages(&self) -> &[CapabilityPageSnapshot] {
        &self.pages
    }

    pub fn mode01_pages(&self) -> &[CapabilityPageSnapshot] {
        self.pages()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TargetMappingSnapshot {
    role: Option<RoleAssignment>,
    responder: Option<ResponderIdentity>,
    target: RequestTarget,
    provenance: Provenance,
}

impl TargetMappingSnapshot {
    pub fn new(
        role: Option<RoleAssignment>,
        responder: Option<ResponderIdentity>,
        target: RequestTarget,
        provenance: Provenance,
    ) -> Self {
        Self {
            role,
            responder,
            target,
            provenance,
        }
    }

    pub fn role(&self) -> Option<&RoleAssignment> {
        self.role.as_ref()
    }

    pub fn responder(&self) -> Option<&ResponderIdentity> {
        self.responder.as_ref()
    }

    pub fn target(&self) -> &RequestTarget {
        &self.target
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub const fn confidence(&self) -> Confidence {
        self.provenance.confidence()
    }

    pub fn to_vehicle_knowledge_mapping(
        &self,
    ) -> Option<crate::vehicle_knowledge::EcuTargetMapping> {
        Some(crate::vehicle_knowledge::EcuTargetMapping::new(
            self.role.clone()?,
            RequestTargetEvidence::new(self.target.clone(), self.provenance.clone()),
            self.responder.clone()?,
        ))
    }
}

/// A bounded, typed value used for cache validation.  It intentionally omits
/// timestamps, identity and historical evidence.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ValidationSignature {
    topology: Vec<TopologyObservation>,
    ecu_capabilities: Vec<EcuCapabilitySnapshot>,
    target_mappings: Vec<TargetMappingSnapshot>,
}

impl ValidationSignature {
    pub fn topology(&self) -> &[TopologyObservation] {
        &self.topology
    }

    pub fn ecu_capabilities(&self) -> &[EcuCapabilitySnapshot] {
        &self.ecu_capabilities
    }

    pub fn target_mappings(&self) -> &[TargetMappingSnapshot] {
        &self.target_mappings
    }

    pub(crate) fn entries(&self) -> Vec<String> {
        let mut entries = self
            .topology
            .iter()
            .map(|observation| format!("topology:{observation:?}"))
            .chain(
                self.ecu_capabilities
                    .iter()
                    .map(|capability| format!("capability:{capability:?}")),
            )
            .chain(
                self.target_mappings
                    .iter()
                    .map(|mapping| format!("target_mapping:{mapping:?}")),
            )
            .collect::<Vec<_>>();
        entries.sort_unstable();
        entries
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
        validate_record(cache)?;
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
            for observation in &cache.snapshot.topology {
                file.write_all(b"\ntopology\t")?;
                file.write_all(encode_topology_observation(observation).as_bytes())?;
            }
            for (index, capability) in cache.snapshot.ecu_capabilities.iter().enumerate() {
                file.write_all(b"\ncapability\t")?;
                file.write_all(index.to_string().as_bytes())?;
                file.write_all(b"\t")?;
                file.write_all(
                    encode_fields(&encode_responder_fields(capability.responder())).as_bytes(),
                )?;
                for page in &capability.pages {
                    file.write_all(b"\ncapability_page\t")?;
                    file.write_all(index.to_string().as_bytes())?;
                    file.write_all(b"\t")?;
                    file.write_all(encode_capability_page(page).as_bytes())?;
                }
            }
            for mapping in &cache.snapshot.target_mappings {
                file.write_all(b"\ntarget_mapping\t")?;
                file.write_all(encode_target_mapping(mapping).as_bytes())?;
            }
            for observation in &cache.snapshot.ecu_identification {
                file.write_all(b"\necu_identification\t")?;
                file.write_all(encode_identification_observation(observation).as_bytes())?;
            }
            for line in &cache.history {
                file.write_all(b"\nhistory\t")?;
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

fn encode_topology_observation(observation: &TopologyObservation) -> String {
    let mut fields = encode_context(observation.context()).to_vec();
    fields.extend(encode_responder_fields(observation.responder()));
    fields.push(
        if observation.payload.is_some() {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    if let Some(payload) = observation.payload() {
        fields.push(hex(payload));
    }
    fields.push(
        if observation.observation.is_some() {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    if let Some(window) = observation.observation {
        fields.push(window.first_observed_ms().to_string());
        fields.push(window.last_observed_ms().to_string());
    }
    fields.extend(encode_provenance(observation.provenance()));
    encode_fields(&fields)
}

fn encode_capability_page(page: &CapabilityPageSnapshot) -> String {
    let fields = [
        page.request[0].to_string(),
        page.request[1].to_string(),
        hex(&page.payload),
        page.provenance.source().into(),
        encode_confidence(page.provenance.confidence()).into(),
    ];
    encode_fields(&fields)
}

fn encode_target_mapping(mapping: &TargetMappingSnapshot) -> String {
    let mut fields = vec![if mapping.role.is_some() { "1" } else { "0" }.into()];
    if let Some(role) = mapping.role() {
        fields.extend(encode_role_assignment(role));
    }
    fields.push(
        if mapping.responder.is_some() {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    if let Some(responder) = mapping.responder() {
        fields.extend(encode_responder_fields(responder));
    }
    fields.extend(encode_target(mapping.target()));
    fields.extend(encode_provenance(mapping.provenance()));
    encode_fields(&fields)
}

fn encode_identification_observation(observation: &IdentificationObservation) -> String {
    let mut fields = encode_target(observation.target());
    fields.extend(encode_responder_fields(observation.expected_responder()));
    fields.extend([
        observation.semantic().into(),
        observation.definition_id().into(),
        observation.definition_version().to_string(),
        observation.knowledge_repository().into(),
        observation.knowledge_revision().into(),
        hex(&observation.request()),
        encode_identification_status(observation.status()).into(),
        if observation.nrc().is_some() {
            "1"
        } else {
            "0"
        }
        .into(),
    ]);
    if let Some(nrc) = observation.nrc() {
        fields.push(nrc.to_string());
    }
    fields.push(
        if observation.value().is_some() {
            "1"
        } else {
            "0"
        }
        .into(),
    );
    if let Some(value) = observation.value() {
        fields.push(hex(value));
    }
    fields.push(observation.responses().len().to_string());
    for response in observation.responses() {
        fields.push(
            if response.responder().is_some() {
                "1"
            } else {
                "0"
            }
            .into(),
        );
        if let Some(responder) = response.responder() {
            fields.extend(encode_responder_fields(responder));
        }
        fields.push(hex(response.payload()));
    }
    fields.push(observation.errors().len().to_string());
    fields.extend(observation.errors().iter().cloned());
    encode_fields(&fields)
}

fn encode_identification_status(status: IdentificationResultStatus) -> &'static str {
    match status {
        IdentificationResultStatus::Supported => "supported",
        IdentificationResultStatus::Unsupported => "unsupported",
        IdentificationResultStatus::NegativeResponse => "negative_response",
        IdentificationResultStatus::Unavailable => "unavailable",
        IdentificationResultStatus::Malformed => "malformed",
        IdentificationResultStatus::Timeout => "timeout",
        IdentificationResultStatus::TransportError => "transport_error",
        IdentificationResultStatus::NotProbed => "not_probed",
    }
}

fn encode_role_assignment(role: &RoleAssignment) -> Vec<String> {
    let mut fields = vec![encode_role(role.role())];
    fields.extend(encode_provenance(role.provenance()));
    fields
}

fn encode_role(role: &EcuRole) -> String {
    match role {
        EcuRole::Engine => "engine".into(),
        EcuRole::Transmission => "transmission".into(),
        EcuRole::Gateway => "gateway".into(),
        EcuRole::Unknown => "unknown".into(),
        EcuRole::VendorSpecific(value) => format!("vendor:{value}"),
    }
}

fn encode_context(context: &ProtocolContext) -> [String; 2] {
    [
        encode_protocol(context.protocol()),
        encode_addressing(context.addressing()),
    ]
}

fn encode_responder_fields(responder: &ResponderIdentity) -> Vec<String> {
    let context = encode_context(responder.context());
    let (kind, value) = match responder {
        ResponderIdentity::Address { value, .. } => ("address", value.as_str()),
        ResponderIdentity::Opaque { value, .. } => ("opaque", value.as_str()),
        ResponderIdentity::Unknown { .. } => ("unknown", ""),
    };
    vec![
        context[0].clone(),
        context[1].clone(),
        kind.into(),
        value.into(),
    ]
}

fn encode_target(target: &RequestTarget) -> Vec<String> {
    let context = encode_context(target.context());
    let mut fields = vec![context[0].clone(), context[1].clone()];
    if let Some(address) = target.address() {
        fields.extend([
            "1".into(),
            address.namespace().into(),
            address.value().into(),
        ]);
    } else {
        fields.push("0".into());
    }
    fields
}

fn encode_provenance(provenance: &Provenance) -> [String; 2] {
    [
        provenance.source().into(),
        encode_confidence(provenance.confidence()).into(),
    ]
}

fn encode_protocol(protocol: &Protocol) -> String {
    match protocol {
        Protocol::Obd2 => "obd2".into(),
        Protocol::Uds => "uds".into(),
        Protocol::Can => "can".into(),
        Protocol::Doip => "doip".into(),
        Protocol::Unknown => "unknown".into(),
        Protocol::VendorSpecific(value) => format!("vendor:{value}"),
    }
}

fn encode_addressing(addressing: &AddressingContext) -> String {
    match addressing {
        AddressingContext::Functional => "functional".into(),
        AddressingContext::Physical => "physical".into(),
        AddressingContext::Unknown => "unknown".into(),
        AddressingContext::VendorSpecific(value) => format!("vendor:{value}"),
    }
}

fn encode_confidence(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Unknown => "unknown",
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
        Confidence::Verified => "verified",
    }
}

fn encode_fields(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| escape(field))
        .collect::<Vec<_>>()
        .join("\t")
}

fn parse_fields(line: &str) -> Result<(&str, Vec<String>), String> {
    let mut fields = line.split('\t');
    let tag = fields.next().unwrap_or_default();
    if tag.is_empty() {
        return Err("malformed vehicle cache field".into());
    }
    Ok((tag, fields.map(unescape).collect::<Result<Vec<_>, _>>()?))
}

fn field<'a>(fields: &'a [String], index: &mut usize, name: &str) -> Result<&'a str, String> {
    let value = fields
        .get(*index)
        .ok_or_else(|| format!("vehicle cache {name} is missing"))?;
    *index += 1;
    Ok(value)
}

fn finish_fields(fields: &[String], index: usize) -> Result<(), String> {
    if index == fields.len() {
        Ok(())
    } else {
        Err("vehicle cache contains an extra field".into())
    }
}

fn parse_context(fields: &[String], index: &mut usize) -> Result<ProtocolContext, String> {
    Ok(ProtocolContext::new(
        parse_protocol(field(fields, index, "protocol")?)?,
        parse_addressing(field(fields, index, "addressing")?)?,
    ))
}

fn parse_responder(fields: &[String], index: &mut usize) -> Result<ResponderIdentity, String> {
    let context = parse_context(fields, index)?;
    let kind = field(fields, index, "responder kind")?;
    let value = field(fields, index, "responder value")?;
    match kind {
        "address" => Ok(ResponderIdentity::address(context, value)),
        "opaque" => Ok(ResponderIdentity::opaque(context, value)),
        "unknown" if value.is_empty() => Ok(ResponderIdentity::unknown(context)),
        _ => Err("vehicle cache contains an invalid responder kind".into()),
    }
}

fn parse_provenance(fields: &[String], index: &mut usize) -> Result<Provenance, String> {
    let source = field(fields, index, "provenance source")?;
    let confidence = parse_confidence(field(fields, index, "confidence")?)?;
    Provenance::new(source, confidence).map_err(|error| error.to_string())
}

fn parse_protocol(value: &str) -> Result<Protocol, String> {
    Ok(match value {
        "obd2" => Protocol::Obd2,
        "uds" => Protocol::Uds,
        "can" => Protocol::Can,
        "doip" => Protocol::Doip,
        "unknown" => Protocol::Unknown,
        value => value
            .strip_prefix("vendor:")
            .map(|value| Protocol::VendorSpecific(value.into()))
            .ok_or_else(|| "vehicle cache contains an invalid protocol".to_string())?,
    })
}

fn parse_addressing(value: &str) -> Result<AddressingContext, String> {
    Ok(match value {
        "functional" => AddressingContext::Functional,
        "physical" => AddressingContext::Physical,
        "unknown" => AddressingContext::Unknown,
        value => value
            .strip_prefix("vendor:")
            .map(|value| AddressingContext::VendorSpecific(value.into()))
            .ok_or_else(|| "vehicle cache contains an invalid addressing context".to_string())?,
    })
}

fn parse_confidence(value: &str) -> Result<Confidence, String> {
    match value {
        "unknown" => Ok(Confidence::Unknown),
        "low" => Ok(Confidence::Low),
        "medium" => Ok(Confidence::Medium),
        "high" => Ok(Confidence::High),
        "verified" => Ok(Confidence::Verified),
        _ => Err("vehicle cache contains an invalid confidence".into()),
    }
}

fn parse_bytes(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("vehicle cache contains an odd-length byte string".into());
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "vehicle cache contains invalid hexadecimal bytes".to_string())
        })
        .collect()
}

fn parse_optional_flag(fields: &[String], index: &mut usize, name: &str) -> Result<bool, String> {
    match field(fields, index, name)? {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("vehicle cache {name} flag is invalid")),
    }
}

fn parse_target(fields: &[String], index: &mut usize) -> Result<RequestTarget, String> {
    let context = parse_context(fields, index)?;
    let has_address = parse_optional_flag(fields, index, "target address")?;
    let address = if has_address {
        Some(RequestAddress::new(
            field(fields, index, "target namespace")?,
            field(fields, index, "target value")?,
        ))
    } else {
        None
    };
    Ok(match address {
        Some(address) => RequestTarget::concrete(context, address),
        None => RequestTarget::functional(context),
    })
}

fn parse(contents: &str, requested_key: &str) -> Result<VehicleCache, String> {
    let mut lines = contents.lines();
    match lines.next() {
        Some(HEADER) => parse_v4(lines, requested_key),
        Some(V3_HEADER) => parse_v3(lines, requested_key),
        Some(V2_HEADER) => parse_v2(lines, requested_key),
        Some(LEGACY_HEADER) => parse_v1(lines, requested_key),
        _ => Err("unsupported vehicle cache format".into()),
    }
}

fn parse_v1<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    let mut local_key = None;
    let mut first_seen_ms = None;
    let mut last_seen_ms = None;
    let mut history = Vec::new();
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
                first_seen_ms = Some(parse_timestamp(encoded, "first_seen_ms")?);
            }
            "last_seen_ms" => {
                if last_seen_ms.is_some() {
                    return Err("vehicle cache contains duplicate last_seen_ms".into());
                }
                last_seen_ms = Some(parse_timestamp(encoded, "last_seen_ms")?);
            }
            "evidence" => history.push(unescape(encoded)?),
            _ => return Err(format!("vehicle cache contains unsupported field {field}")),
        }
    }
    let cache = VehicleCache::new(
        local_key.ok_or_else(|| "vehicle cache is missing local_key".to_string())?,
        first_seen_ms.ok_or_else(|| "vehicle cache is missing first_seen_ms".to_string())?,
        last_seen_ms.ok_or_else(|| "vehicle cache is missing last_seen_ms".to_string())?,
        history,
    );
    finish_cache(cache, requested_key)
}

fn parse_v2<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, false, false)
}

fn parse_v3<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, true, false)
}

fn parse_v4<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
) -> Result<VehicleCache, String> {
    parse_versioned(lines, requested_key, true, true)
}

fn parse_versioned<'a>(
    lines: impl Iterator<Item = &'a str>,
    requested_key: &str,
    has_role: bool,
    has_identification: bool,
) -> Result<VehicleCache, String> {
    let mut local_key = None;
    let mut first_seen_ms = None;
    let mut last_seen_ms = None;
    let mut topology = Vec::new();
    let mut capabilities = BTreeMap::new();
    let mut target_mappings = Vec::new();
    let mut ecu_identification = Vec::new();
    let mut history = Vec::new();
    for line in lines {
        let (tag, fields) = parse_fields(line)?;
        match tag {
            "local_key" => {
                if local_key.is_some() {
                    return Err("vehicle cache contains duplicate local_key".into());
                }
                local_key = Some(one_field(&fields, "local_key")?.to_owned());
            }
            "first_seen_ms" => {
                if first_seen_ms.is_some() {
                    return Err("vehicle cache contains duplicate first_seen_ms".into());
                }
                first_seen_ms = Some(parse_timestamp(
                    one_field(&fields, "first_seen_ms")?,
                    "first_seen_ms",
                )?);
            }
            "last_seen_ms" => {
                if last_seen_ms.is_some() {
                    return Err("vehicle cache contains duplicate last_seen_ms".into());
                }
                last_seen_ms = Some(parse_timestamp(
                    one_field(&fields, "last_seen_ms")?,
                    "last_seen_ms",
                )?);
            }
            "topology" => topology.push(parse_topology_observation(&fields)?),
            "capability" => {
                let mut index = 0;
                let capability_index = field(&fields, &mut index, "capability index")?
                    .parse::<usize>()
                    .map_err(|error| format!("invalid capability index: {error}"))?;
                let responder = parse_responder(&fields, &mut index)?;
                finish_fields(&fields, index)?;
                if capabilities
                    .insert(capability_index, EcuCapabilitySnapshot::new(responder, []))
                    .is_some()
                {
                    return Err("vehicle cache contains duplicate capability".into());
                }
            }
            "capability_page" => {
                let mut index = 0;
                let capability_index = field(&fields, &mut index, "capability index")?
                    .parse::<usize>()
                    .map_err(|error| format!("invalid capability index: {error}"))?;
                let page = parse_capability_page(&fields, &mut index)?;
                finish_fields(&fields, index)?;
                let capability = capabilities.get_mut(&capability_index).ok_or_else(|| {
                    "vehicle cache capability page precedes its capability".to_string()
                })?;
                capability.pages.push(page);
                capability.pages.sort();
            }
            "target_mapping" => {
                target_mappings.push(parse_target_mapping_with_role(&fields, has_role)?);
            }
            "ecu_identification" if has_identification => {
                ecu_identification.push(parse_identification_observation(&fields)?);
            }
            "history" => history.push(one_field(&fields, "history")?.to_owned()),
            "evidence" => history.push(one_field(&fields, "evidence")?.to_owned()),
            _ => return Err(format!("vehicle cache contains unsupported field {tag}")),
        }
    }

    let cache = VehicleCache::new(
        local_key.ok_or_else(|| "vehicle cache is missing local_key".to_string())?,
        first_seen_ms.ok_or_else(|| "vehicle cache is missing first_seen_ms".to_string())?,
        last_seen_ms.ok_or_else(|| "vehicle cache is missing last_seen_ms".to_string())?,
        history,
    );
    let cache = VehicleCache::with_snapshot(
        cache.local_key,
        cache.first_seen_ms,
        cache.last_seen_ms,
        VehicleCacheSnapshot::with_ecu_identification(
            topology,
            capabilities.into_values(),
            target_mappings,
            ecu_identification,
        ),
        cache.history,
    );
    finish_cache(cache, requested_key)
}

fn finish_cache(cache: VehicleCache, requested_key: &str) -> Result<VehicleCache, String> {
    validate_record(&cache)?;
    if cache.local_key != requested_key {
        return Err("vehicle cache local_key does not match its path".into());
    }
    Ok(cache)
}

fn one_field<'a>(fields: &'a [String], name: &str) -> Result<&'a str, String> {
    if fields.len() == 1 {
        Ok(&fields[0])
    } else {
        Err(format!("vehicle cache {name} must contain one field"))
    }
}

fn parse_timestamp(value: &str, name: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| format!("invalid {name}: {error}"))
}

fn parse_topology_observation(fields: &[String]) -> Result<TopologyObservation, String> {
    let mut index = 0;
    let context = parse_context(fields, &mut index)?;
    let responder = parse_responder(fields, &mut index)?;
    let payload = if parse_optional_flag(fields, &mut index, "payload")? {
        Some(parse_bytes(field(fields, &mut index, "payload bytes")?)?)
    } else {
        None
    };
    let observation = if parse_optional_flag(fields, &mut index, "observation")? {
        Some(
            ObservationWindow::new(
                parse_timestamp(
                    field(fields, &mut index, "observation start")?,
                    "observation start",
                )?,
                parse_timestamp(
                    field(fields, &mut index, "observation end")?,
                    "observation end",
                )?,
            )
            .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let provenance = parse_provenance(fields, &mut index)?;
    finish_fields(fields, index)?;
    Ok(TopologyObservation::new(
        context,
        responder,
        payload,
        observation,
        provenance,
    ))
}

fn parse_capability_page(
    fields: &[String],
    index: &mut usize,
) -> Result<CapabilityPageSnapshot, String> {
    let request = [
        field(fields, index, "capability mode")?
            .parse::<u8>()
            .map_err(|error| format!("invalid capability mode: {error}"))?,
        field(fields, index, "capability PID")?
            .parse::<u8>()
            .map_err(|error| format!("invalid capability PID: {error}"))?,
    ];
    let payload = parse_bytes(field(fields, index, "capability payload")?)?;
    let provenance = parse_provenance(fields, index)?;
    Ok(CapabilityPageSnapshot::new(request, payload, provenance))
}

fn parse_target_mapping_with_role(
    fields: &[String],
    has_role: bool,
) -> Result<TargetMappingSnapshot, String> {
    let mut index = 0;
    let role = if has_role && parse_optional_flag(fields, &mut index, "mapping role")? {
        Some(parse_role_assignment(fields, &mut index)?)
    } else {
        None
    };
    let responder = if parse_optional_flag(fields, &mut index, "mapping responder")? {
        Some(parse_responder(fields, &mut index)?)
    } else {
        None
    };
    let target = parse_target(fields, &mut index)?;
    let provenance = parse_provenance(fields, &mut index)?;
    finish_fields(fields, index)?;
    Ok(TargetMappingSnapshot::new(
        role, responder, target, provenance,
    ))
}

fn parse_identification_observation(
    fields: &[String],
) -> Result<IdentificationObservation, String> {
    let mut index = 0;
    let target = parse_target(fields, &mut index)?;
    let expected_responder = parse_responder(fields, &mut index)?;
    let semantic = field(fields, &mut index, "identification semantic")?.to_owned();
    let definition_id = field(fields, &mut index, "identification definition")?.to_owned();
    let definition_version = field(fields, &mut index, "identification definition version")?
        .parse::<u32>()
        .map_err(|error| format!("invalid identification definition version: {error}"))?;
    let knowledge_repository =
        field(fields, &mut index, "identification knowledge repository")?.to_owned();
    let knowledge_revision =
        field(fields, &mut index, "identification knowledge revision")?.to_owned();
    let request_bytes = parse_bytes(field(fields, &mut index, "identification request")?)?;
    let request: [u8; 3] = request_bytes
        .try_into()
        .map_err(|_| "ECU identification request must contain exactly three bytes".to_string())?;
    let status = parse_identification_status(field(fields, &mut index, "identification status")?)?;
    let nrc = if parse_optional_flag(fields, &mut index, "identification NRC")? {
        Some(
            field(fields, &mut index, "identification NRC value")?
                .parse::<u8>()
                .map_err(|error| format!("invalid identification NRC: {error}"))?,
        )
    } else {
        None
    };
    let value = if parse_optional_flag(fields, &mut index, "identification value")? {
        Some(parse_bytes(field(
            fields,
            &mut index,
            "identification value bytes",
        )?)?)
    } else {
        None
    };
    let response_count = field(fields, &mut index, "identification response count")?
        .parse::<usize>()
        .map_err(|error| format!("invalid identification response count: {error}"))?;
    let mut responses = Vec::with_capacity(response_count);
    for _ in 0..response_count {
        let responder = if parse_optional_flag(fields, &mut index, "identification responder")? {
            Some(parse_responder(fields, &mut index)?)
        } else {
            None
        };
        let payload = parse_bytes(field(
            fields,
            &mut index,
            "identification response payload",
        )?)?;
        responses.push(IdentificationResponseEvidence::new(responder, payload));
    }
    let error_count = field(fields, &mut index, "identification error count")?
        .parse::<usize>()
        .map_err(|error| format!("invalid identification error count: {error}"))?;
    let mut errors = Vec::with_capacity(error_count);
    for _ in 0..error_count {
        errors.push(field(fields, &mut index, "identification error")?.to_owned());
    }
    finish_fields(fields, index)?;
    IdentificationObservation::new(
        target,
        expected_responder,
        semantic,
        definition_id,
        definition_version,
        knowledge_repository,
        knowledge_revision,
        request,
        status,
        responses,
        nrc,
        value,
        errors,
    )
}

fn parse_identification_status(value: &str) -> Result<IdentificationResultStatus, String> {
    match value {
        "supported" => Ok(IdentificationResultStatus::Supported),
        "unsupported" => Ok(IdentificationResultStatus::Unsupported),
        "negative_response" => Ok(IdentificationResultStatus::NegativeResponse),
        "unavailable" => Ok(IdentificationResultStatus::Unavailable),
        "malformed" => Ok(IdentificationResultStatus::Malformed),
        "timeout" => Ok(IdentificationResultStatus::Timeout),
        "transport_error" => Ok(IdentificationResultStatus::TransportError),
        "not_probed" => Ok(IdentificationResultStatus::NotProbed),
        _ => Err("vehicle cache contains an invalid ECU identification status".into()),
    }
}

fn parse_role_assignment(fields: &[String], index: &mut usize) -> Result<RoleAssignment, String> {
    let role = parse_role(field(fields, index, "mapping role")?)?;
    let provenance = parse_provenance(fields, index)?;
    Ok(RoleAssignment::new(role, provenance))
}

fn parse_role(value: &str) -> Result<EcuRole, String> {
    Ok(match value {
        "engine" => EcuRole::Engine,
        "transmission" => EcuRole::Transmission,
        "gateway" => EcuRole::Gateway,
        "unknown" => EcuRole::Unknown,
        value => value
            .strip_prefix("vendor:")
            .map(|value| EcuRole::VendorSpecific(value.into()))
            .ok_or_else(|| "vehicle cache contains an invalid ECU role".to_string())?,
    })
}

fn validate_record(cache: &VehicleCache) -> Result<(), String> {
    validate_text("local key", &cache.local_key)?;
    if cache.last_seen_ms < cache.first_seen_ms {
        return Err("vehicle cache last_seen_ms precedes first_seen_ms".into());
    }
    validate_snapshot(&cache.snapshot)?;
    for line in &cache.history {
        validate_text("evidence", line)?;
    }
    Ok(())
}

fn validate_snapshot(snapshot: &VehicleCacheSnapshot) -> Result<(), String> {
    for observation in &snapshot.topology {
        validate_context(observation.context())?;
        validate_responder(observation.responder())?;
        if let Some(payload) = observation.payload() {
            if payload.len() > 4096 {
                return Err("vehicle cache topology payload is too large".into());
            }
        }
        validate_provenance(observation.provenance())?;
    }
    for capability in &snapshot.ecu_capabilities {
        validate_responder(capability.responder())?;
        for page in capability.pages() {
            validate_provenance(page.provenance())?;
            if page.payload().len() > 4096 {
                return Err("vehicle cache capability payload is too large".into());
            }
        }
    }
    for observation in &snapshot.ecu_identification {
        observation.validate()?;
        validate_context(observation.target().context())?;
        if let Some(address) = observation.target().address() {
            validate_text("identification target namespace", address.namespace())?;
            validate_text("identification target value", address.value())?;
        }
        validate_responder(observation.expected_responder())?;
        validate_text("identification semantic", observation.semantic())?;
        validate_text("identification definition", observation.definition_id())?;
        validate_text(
            "identification knowledge repository",
            observation.knowledge_repository(),
        )?;
        validate_knowledge_revision(observation.knowledge_revision())?;
        if observation.value().is_some_and(|value| value.len() > 4096) {
            return Err("vehicle cache ECU identification value is too large".into());
        }
        for response in observation.responses() {
            if let Some(responder) = response.responder() {
                validate_responder(responder)?;
            }
            if response.payload().len() > 4096 {
                return Err("vehicle cache ECU identification response is too large".into());
            }
        }
        for error in observation.errors() {
            validate_text("identification error", error)?;
        }
    }
    for mapping in &snapshot.target_mappings {
        if let Some(role) = mapping.role() {
            validate_role(role)?;
        }
        if let Some(responder) = mapping.responder() {
            validate_responder(responder)?;
        }
        validate_context(mapping.target().context())?;
        if let Some(address) = mapping.target().address() {
            validate_text("target namespace", address.namespace())?;
            validate_text("target value", address.value())?;
        }
        validate_provenance(mapping.provenance())?;
    }
    Ok(())
}

fn validate_role(role: &RoleAssignment) -> Result<(), String> {
    if let EcuRole::VendorSpecific(value) = role.role() {
        validate_text("ECU role", value)?;
    }
    validate_provenance(role.provenance())
}

fn validate_context(context: &ProtocolContext) -> Result<(), String> {
    if let Protocol::VendorSpecific(value) = context.protocol() {
        validate_text("protocol", value)?;
    }
    if let AddressingContext::VendorSpecific(value) = context.addressing() {
        validate_text("addressing", value)?;
    }
    Ok(())
}

fn validate_responder(responder: &ResponderIdentity) -> Result<(), String> {
    validate_context(responder.context())?;
    if let Some(value) = responder.value() {
        validate_text("responder", value)?;
    }
    Ok(())
}

fn validate_provenance(provenance: &Provenance) -> Result<(), String> {
    validate_text("provenance", provenance.source())
}

fn validate_knowledge_revision(value: &str) -> Result<(), String> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("vehicle cache knowledge revision is not a full Git object id".into());
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
            "OBDENTIC-VEHICLE-CACHE\t4\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t2\nhistory\tevidence\n"
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
            "OBDENTIC-VEHICLE-CACHE\t5\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t1\n",
        )
        .unwrap();
        assert!(store.load("local-key").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn round_trips_typed_snapshot_and_keeps_history_separate() {
        let root = root("typed-roundtrip");
        let store = CacheStore::new(&root);
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let provenance = Provenance::new("functional discovery", Confidence::High).unwrap();
        let topology = TopologyObservation::new(
            context.clone(),
            ResponderIdentity::opaque(context.clone(), "7E8"),
            Some(vec![0x41, 0x00, 0, 0, 0, 1]),
            Some(ObservationWindow::new(10, 20).unwrap()),
            provenance.clone(),
        );
        let snapshot = VehicleCacheSnapshot::new(
            [topology],
            [EcuCapabilitySnapshot::new(
                ResponderIdentity::opaque(context.clone(), "7E8"),
                [CapabilityPageSnapshot::new(
                    [0x01, 0x00],
                    vec![0x41, 0x00, 0, 0, 0, 1],
                    provenance.clone(),
                )],
            )],
            [TargetMappingSnapshot::new(
                None,
                Some(ResponderIdentity::opaque(context.clone(), "7E8")),
                RequestTarget::functional(context),
                provenance,
            )],
        );
        let cache = VehicleCache::with_snapshot(
            "local-key",
            10,
            20,
            snapshot.clone(),
            vec!["historical evidence".into()],
        );
        store.save(&cache).unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert_eq!(loaded.snapshot(), &snapshot);
        assert_eq!(loaded.history(), ["historical evidence"]);
        assert_eq!(
            loaded.snapshot().validation_signature(),
            snapshot.validation_signature()
        );
        assert!(loaded
            .snapshot()
            .validation_signature()
            .target_mappings()
            .is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validation_signature_keeps_only_mode_01_page_00_evidence() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let responder = ResponderIdentity::opaque(context.clone(), "7E8");
        let provenance = Provenance::new("functional discovery", Confidence::High).unwrap();
        let snapshot = VehicleCacheSnapshot::new(
            [
                TopologyObservation::new(
                    context.clone(),
                    responder.clone(),
                    Some(vec![0x41, 0x00, 0, 0, 0, 1]),
                    None,
                    provenance.clone(),
                ),
                TopologyObservation::new(
                    context.clone(),
                    responder.clone(),
                    Some(vec![0x41, 0x20, 0, 0, 0, 1]),
                    None,
                    provenance.clone(),
                ),
                TopologyObservation::new(
                    context,
                    responder.clone(),
                    None,
                    None,
                    provenance.clone(),
                ),
            ],
            [EcuCapabilitySnapshot::new(
                responder,
                [
                    CapabilityPageSnapshot::new(
                        [0x01, 0x00],
                        vec![0x41, 0x00, 0, 0, 0, 1],
                        provenance.clone(),
                    ),
                    CapabilityPageSnapshot::new(
                        [0x01, 0x20],
                        vec![0x41, 0x20, 0, 0, 0, 1],
                        provenance,
                    ),
                ],
            )],
            [],
        );

        let signature = snapshot.validation_signature();
        assert_eq!(signature.topology().len(), 1);
        assert_eq!(signature.ecu_capabilities().len(), 1);
        assert_eq!(signature.ecu_capabilities()[0].pages().len(), 1);
        assert_eq!(
            signature.ecu_capabilities()[0].pages()[0].request(),
            [0x01, 0x00]
        );
        assert!(signature.target_mappings().is_empty());
    }

    #[test]
    fn only_roleful_mapping_can_be_reconstructed_for_routing() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let provenance = Provenance::new("validated topology", Confidence::High).unwrap();
        let mapping = TargetMappingSnapshot::new(
            Some(RoleAssignment::new(EcuRole::Engine, provenance.clone())),
            Some(ResponderIdentity::address(context.clone(), "7E8")),
            RequestTarget::concrete(context, RequestAddress::new("elm-header", "7E0")),
            provenance,
        );
        assert_eq!(
            mapping
                .to_vehicle_knowledge_mapping()
                .unwrap()
                .role()
                .role(),
            &EcuRole::Engine
        );
        assert!(TargetMappingSnapshot::new(
            None,
            mapping.responder().cloned(),
            mapping.target().clone(),
            mapping.provenance().clone(),
        )
        .to_vehicle_knowledge_mapping()
        .is_none());
    }

    #[test]
    fn round_trips_per_ecu_identification_without_affecting_validation_signature() {
        let root = root("ecu-identification");
        let store = CacheStore::new(&root);
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let target =
            RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0"));
        let responder = ResponderIdentity::address(context, "7E8");
        let supported = IdentificationObservation::new(
            target.clone(),
            responder.clone(),
            "ecu.manufacturer_software_version",
            "uds.f189.manufacturer_software_version",
            1,
            "frankherchet/obdentic-knowledge",
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
            [0x22, 0xF1, 0x89],
            IdentificationResultStatus::Supported,
            vec![IdentificationResponseEvidence::new(
                Some(responder.clone()),
                vec![0x62, 0xF1, 0x89, 0x31, 0x2E],
            )],
            None,
            Some(vec![0x31, 0x2E]),
            Vec::new(),
        )
        .unwrap();
        let timeout = IdentificationObservation::new(
            target,
            responder,
            "ecu.boot_software_identification",
            "uds.f180.boot_software_identification",
            1,
            "frankherchet/obdentic-knowledge",
            "661fba8eed8ddce8fef5bba4c68dfcba85e2dd28",
            [0x22, 0xF1, 0x80],
            IdentificationResultStatus::Timeout,
            Vec::new(),
            None,
            None,
            vec!["Carly command timed out".into()],
        )
        .unwrap();
        let snapshot =
            VehicleCacheSnapshot::with_ecu_identification([], [], [], [supported, timeout]);
        let signature = snapshot.validation_signature();
        assert!(signature.topology().is_empty());
        assert!(signature.ecu_capabilities().is_empty());
        assert!(signature.target_mappings().is_empty());

        store
            .save(&VehicleCache::with_snapshot(
                "local-key",
                1,
                2,
                snapshot.clone(),
                Vec::new(),
            ))
            .unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert_eq!(loaded.snapshot(), &snapshot);
        assert_eq!(
            loaded
                .snapshot()
                .ecu_identification()
                .iter()
                .map(IdentificationObservation::status)
                .collect::<Vec<_>>(),
            vec![
                IdentificationResultStatus::Timeout,
                IdentificationResultStatus::Supported,
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_v3_cache_without_ecu_identification() {
        let root = root("v3");
        let store = CacheStore::new(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("6c6f63616c2d6b6579.tsv"),
            "OBDENTIC-VEHICLE-CACHE\t3\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t1\n",
        )
        .unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert!(loaded.snapshot().ecu_identification().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_vin_did_in_ecu_identification_cache() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        let observation = IdentificationObservation::new(
            RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0")),
            ResponderIdentity::address(context, "7E8"),
            "ecu.vin",
            "uds.f190.vin",
            1,
            "frankherchet/obdentic-knowledge",
            "revision",
            [0x22, 0xF1, 0x90],
            IdentificationResultStatus::NotProbed,
            Vec::new(),
            None,
            None,
            Vec::new(),
        );
        assert!(observation.is_err());
    }

    #[test]
    fn loads_legacy_textual_evidence_as_history_only() {
        let root = root("legacy");
        let store = CacheStore::new(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("6c6f63616c2d6b6579.tsv"),
            "OBDENTIC-VEHICLE-CACHE\t1\nlocal_key\tlocal-key\nfirst_seen_ms\t1\nlast_seen_ms\t1\nevidence\told\n",
        )
        .unwrap();
        let loaded = store.load("local-key").unwrap().unwrap();
        assert!(loaded.snapshot().topology().is_empty());
        assert_eq!(loaded.history(), ["old"]);
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
