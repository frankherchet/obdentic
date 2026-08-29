//! Evidence-gated VW EA189 vehicle knowledge.
//!
//! The profile contains no guessed ECU address and no manufacturer-specific
//! request.  It can only bind an existing generic, read-only semantic to
//! target evidence supplied by discovery or a validated cache.

use std::collections::BTreeMap;

use crate::{
    diagnostic_job::{DiagnosticScope, EcuRole as JobEcuRole, KnownTarget},
    topology::{AddressingContext, Confidence, EcuRole, Protocol, Provenance},
    vehicle_knowledge::EcuTargetMapping,
    ReadRequest,
};

/// Stable identifier for the first VW vehicle profile.
pub const PROFILE_ID: &str = "vw-ea189-v1";

/// Human-readable platform identity.  This is intentionally not a VIN or a
/// vehicle-specific identity.
pub const PLATFORM: &str = "VW EA189";

/// One candidate UDS ReadDataByIdentifier definition admitted by the closed
/// Gate-A DPF probe.  The definition intentionally contains no interpretation
/// of the returned payload.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ea189DpfDid {
    semantic: &'static str,
    id: u16,
    request_bytes: [u8; 3],
}

impl Ea189DpfDid {
    pub const fn semantic(self) -> &'static str {
        self.semantic
    }

    pub const fn id(self) -> u16 {
        self.id
    }

    pub const fn request_bytes(self) -> [u8; 3] {
        self.request_bytes
    }
}

const DPF_SOOT_MASS_MEASURED: Ea189DpfDid = Ea189DpfDid {
    semantic: "dpf.soot_mass_measured",
    id: 0x114e,
    request_bytes: [0x22, 0x11, 0x4e],
};
const DPF_SOOT_MASS_CALCULATED: Ea189DpfDid = Ea189DpfDid {
    semantic: "dpf.soot_mass_calculated",
    id: 0x114f,
    request_bytes: [0x22, 0x11, 0x4f],
};
const DPF_DISTANCE_SINCE_REGENERATION: Ea189DpfDid = Ea189DpfDid {
    semantic: "dpf.distance_since_regeneration",
    id: 0x1156,
    request_bytes: [0x22, 0x11, 0x56],
};
const DPF_TIME_SINCE_REGENERATION: Ea189DpfDid = Ea189DpfDid {
    semantic: "dpf.time_since_regeneration",
    id: 0x115e,
    request_bytes: [0x22, 0x11, 0x5e],
};
const DPF_PRE_TEMPERATURE: Ea189DpfDid = Ea189DpfDid {
    semantic: "exhaust.temperature.pre_dpf",
    id: 0x11b2,
    request_bytes: [0x22, 0x11, 0xb2],
};
const DPF_POST_TEMPERATURE: Ea189DpfDid = Ea189DpfDid {
    semantic: "exhaust.temperature.post_dpf",
    id: 0x10f9,
    request_bytes: [0x22, 0x10, 0xf9],
};
const DPF_DIFFERENTIAL_PRESSURE: Ea189DpfDid = Ea189DpfDid {
    semantic: "dpf.differential_pressure",
    id: 0x14f5,
    request_bytes: [0x22, 0x14, 0xf5],
};

/// Exactly the seven bounded EA189 DPF candidates admitted for Gate A.
pub const EA189_DPF_DIDS: [Ea189DpfDid; 7] = [
    DPF_SOOT_MASS_CALCULATED,
    DPF_SOOT_MASS_MEASURED,
    DPF_DISTANCE_SINCE_REGENERATION,
    DPF_TIME_SINCE_REGENERATION,
    DPF_PRE_TEMPERATURE,
    DPF_POST_TEMPERATURE,
    DPF_DIFFERENTIAL_PRESSURE,
];

/// A closed selection from the Gate-A DPF candidate set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ea189DpfProbe {
    SootMassMeasured,
    SootMassCalculated,
    DistanceSinceRegeneration,
    TimeSinceRegeneration,
    PreDpfTemperature,
    PostDpfTemperature,
    DifferentialPressure,
}

impl Ea189DpfProbe {
    pub const ALL: [Self; 7] = [
        Self::SootMassCalculated,
        Self::SootMassMeasured,
        Self::DistanceSinceRegeneration,
        Self::TimeSinceRegeneration,
        Self::PreDpfTemperature,
        Self::PostDpfTemperature,
        Self::DifferentialPressure,
    ];

    pub const fn definition(self) -> Ea189DpfDid {
        match self {
            Self::SootMassMeasured => DPF_SOOT_MASS_MEASURED,
            Self::SootMassCalculated => DPF_SOOT_MASS_CALCULATED,
            Self::DistanceSinceRegeneration => DPF_DISTANCE_SINCE_REGENERATION,
            Self::TimeSinceRegeneration => DPF_TIME_SINCE_REGENERATION,
            Self::PreDpfTemperature => DPF_PRE_TEMPERATURE,
            Self::PostDpfTemperature => DPF_POST_TEMPERATURE,
            Self::DifferentialPressure => DPF_DIFFERENTIAL_PRESSURE,
        }
    }

    pub const fn semantic(self) -> &'static str {
        self.definition().semantic()
    }

    pub const fn id(self) -> u16 {
        self.definition().id()
    }

    pub const fn request_bytes(self) -> [u8; 3] {
        self.definition().request_bytes()
    }
}

/// A value decoded from a Gate-A response using a still-experimental EA189
/// hypothesis.  This type is deliberately separate from [`Ea189Profile`]: a
/// successful decode does not promote a candidate into production knowledge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ea189ExperimentalDpfReading {
    probe: Ea189DpfProbe,
    value: f64,
    unit: &'static str,
    decoder: &'static str,
}

impl Ea189ExperimentalDpfReading {
    pub const fn probe(self) -> Ea189DpfProbe {
        self.probe
    }

    pub const fn semantic(self) -> &'static str {
        self.probe.semantic()
    }

    pub const fn did(self) -> u16 {
        self.probe.id()
    }

    pub const fn value(self) -> f64 {
        self.value
    }

    pub const fn unit(self) -> &'static str {
        self.unit
    }

    pub const fn decoder(self) -> &'static str {
        self.decoder
    }

    /// Every reading returned by this module is experimental by construction.
    pub const fn is_experimental(self) -> bool {
        true
    }
}

/// Errors from the closed experimental Gate-B decoder set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ea189DpfDecodeError {
    UnsupportedProbe(Ea189DpfProbe),
    MalformedPositiveResponse,
    WrongDid { expected: u16, actual: u16 },
}

impl std::fmt::Display for Ea189DpfDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProbe(probe) => {
                write!(
                    formatter,
                    "no experimental decoder for DID {:04X}",
                    probe.id()
                )
            }
            Self::MalformedPositiveResponse => {
                formatter.write_str("malformed UDS positive DPF response")
            }
            Self::WrongDid { expected, actual } => write!(
                formatter,
                "UDS response DID {:04X} does not match requested DID {:04X}",
                actual, expected
            ),
        }
    }
}

impl std::error::Error for Ea189DpfDecodeError {}

/// Decode one exact, positive Gate-A response with an experimental hypothesis.
///
/// The accepted shape is exactly `62 <DID_hi> <DID_lo> <A> <B>`.  The three
/// remaining Gate-A candidates have no decoder yet and are rejected.
pub fn decode_experimental_dpf(
    probe: Ea189DpfProbe,
    response: &[u8],
) -> Result<Ea189ExperimentalDpfReading, Ea189DpfDecodeError> {
    let expected_did = probe.id();
    if response.len() != 5 {
        return Err(Ea189DpfDecodeError::MalformedPositiveResponse);
    }
    if response[0] != 0x62 {
        return Err(Ea189DpfDecodeError::MalformedPositiveResponse);
    }
    let actual_did = u16::from_be_bytes([response[1], response[2]]);
    if actual_did != expected_did {
        return Err(Ea189DpfDecodeError::WrongDid {
            expected: expected_did,
            actual: actual_did,
        });
    }
    let data = u16::from_be_bytes([response[3], response[4]]);
    let (value, unit, decoder) = match probe {
        Ea189DpfProbe::SootMassCalculated => {
            (data as f64 / 100.0, "g", "BE u16 / 100 (experimental)")
        }
        Ea189DpfProbe::SootMassMeasured => (
            i16::from_be_bytes([response[3], response[4]]) as f64 / 100.0,
            "g",
            "BE i16 / 100 (experimental)",
        ),
        Ea189DpfProbe::PreDpfTemperature | Ea189DpfProbe::PostDpfTemperature => (
            (i32::from(data) - 2731) as f64 / 10.0,
            "°C",
            "(BE u16 - 2731) / 10 (Kelvin hypothesis; experimental)",
        ),
        Ea189DpfProbe::DistanceSinceRegeneration
        | Ea189DpfProbe::TimeSinceRegeneration
        | Ea189DpfProbe::DifferentialPressure => {
            return Err(Ea189DpfDecodeError::UnsupportedProbe(probe));
        }
    };

    Ok(Ea189ExperimentalDpfReading {
        probe,
        value,
        unit,
        decoder,
    })
}

/// Target-bound DPF probe input. Construction only accepts an engine
/// `KnownEcu` scope, so a candidate cannot be detached into a vehicle-wide or
/// functional operation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ea189DpfProbeRequest {
    probe: Ea189DpfProbe,
    scope: DiagnosticScope,
}

impl Ea189DpfProbeRequest {
    pub fn for_engine(probe: Ea189DpfProbe, target: KnownTarget) -> Self {
        Self {
            probe,
            scope: DiagnosticScope::known_ecu(JobEcuRole::Engine, target),
        }
    }

    pub fn from_scope(
        probe: Ea189DpfProbe,
        scope: DiagnosticScope,
    ) -> Result<Self, Ea189DpfProbeError> {
        match &scope {
            DiagnosticScope::KnownEcu {
                role: JobEcuRole::Engine,
                ..
            } => Ok(Self { probe, scope }),
            _ => Err(Ea189DpfProbeError::RequiresEngineKnownEcu),
        }
    }

    pub const fn probe(&self) -> Ea189DpfProbe {
        self.probe
    }

    pub fn scope(&self) -> &DiagnosticScope {
        &self.scope
    }

    pub fn target(&self) -> &KnownTarget {
        match &self.scope {
            DiagnosticScope::KnownEcu { target, .. } => target,
            _ => unreachable!("EA189 DPF probe scope is engine KnownEcu"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ea189DpfProbeError {
    RequiresEngineKnownEcu,
}

impl std::fmt::Display for Ea189DpfProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EA189 DPF probe requires a known engine ECU target")
    }
}

impl std::error::Error for Ea189DpfProbeError {}

/// Identity of the platform knowledge, independent of any one vehicle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ea189ProfileIdentity {
    id: &'static str,
    platform: &'static str,
    engine_family: &'static str,
}

impl Ea189ProfileIdentity {
    pub const fn id(self) -> &'static str {
        self.id
    }

    pub const fn platform(self) -> &'static str {
        self.platform
    }

    pub const fn engine_family(self) -> &'static str {
        self.engine_family
    }
}

/// The EA189 identity contains only platform-level knowledge.
pub const fn identity() -> Ea189ProfileIdentity {
    Ea189ProfileIdentity {
        id: PROFILE_ID,
        platform: PLATFORM,
        engine_family: "EA189",
    }
}

/// Evidence attached to a profile binding.  Target provenance remains on the
/// supplied [`EcuTargetMapping`]; this provenance describes why the generic
/// semantic is admitted to this profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEvidence {
    provenance: Provenance,
    hardware_validation: String,
}

impl ProfileEvidence {
    pub fn new(
        provenance: Provenance,
        hardware_validation: impl Into<String>,
    ) -> Result<Self, Ea189ProfileError> {
        let hardware_validation = hardware_validation.into();
        if hardware_validation.trim().is_empty() {
            return Err(Ea189ProfileError::MissingHardwareValidation);
        }
        Ok(Self {
            provenance,
            hardware_validation,
        })
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub const fn confidence(&self) -> Confidence {
        self.provenance.confidence()
    }

    pub fn hardware_validation(&self) -> &str {
        &self.hardware_validation
    }
}

/// One valid semantic binding exposed by an [`Ea189Profile`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ea189SignalBinding {
    request: ReadRequest,
    mapping: EcuTargetMapping,
    evidence: ProfileEvidence,
}

impl Ea189SignalBinding {
    pub fn semantic(&self) -> &'static str {
        self.request.metadata().semantic
    }

    pub const fn request(&self) -> ReadRequest {
        self.request
    }

    pub fn metadata(&self) -> &'static crate::SignalMetadata {
        self.request.metadata()
    }

    pub fn target_mapping(&self) -> &EcuTargetMapping {
        &self.mapping
    }

    pub fn evidence(&self) -> &ProfileEvidence {
        &self.evidence
    }
}

/// Small, empty-by-default EA189 profile.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Ea189Profile {
    bindings: BTreeMap<String, Ea189SignalBinding>,
}

impl Ea189Profile {
    pub const fn new() -> Self {
        Self {
            bindings: BTreeMap::new(),
        }
    }

    pub const fn identity(&self) -> Ea189ProfileIdentity {
        identity()
    }

    /// Bind an existing generic semantic to independently supplied,
    /// validated physical target evidence.
    pub fn bind_generic_signal(
        &mut self,
        semantic: &str,
        mapping: EcuTargetMapping,
        evidence: ProfileEvidence,
    ) -> Result<(), Ea189ProfileError> {
        let request = crate::prepare_read(semantic)
            .map_err(|_| Ea189ProfileError::UnsupportedSemantic(semantic.to_owned()))?;
        if request.metadata().profile != "obd2-v1" {
            return Err(Ea189ProfileError::NotGenericSignal(semantic.to_owned()));
        }
        validate_mapping(&mapping)?;
        if self.bindings.contains_key(semantic) {
            return Err(Ea189ProfileError::DuplicateSemantic(semantic.to_owned()));
        }
        self.bindings.insert(
            semantic.to_owned(),
            Ea189SignalBinding {
                request,
                mapping,
                evidence,
            },
        );
        Ok(())
    }

    pub fn binding(&self, semantic: &str) -> Option<&Ea189SignalBinding> {
        self.bindings.get(semantic)
    }

    /// Only externally evidenced bindings are visible as EA189 signals.
    pub fn signals(&self) -> impl Iterator<Item = &Ea189SignalBinding> {
        self.bindings.values()
    }
}

fn validate_mapping(mapping: &EcuTargetMapping) -> Result<(), Ea189ProfileError> {
    let target = mapping.target().target();
    if target.context().protocol() != &Protocol::Obd2
        || target.context().addressing() != &AddressingContext::Physical
    {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require physical OBD-II target evidence".into(),
        ));
    }
    if target.address().is_none() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require a concrete request target".into(),
        ));
    }
    if mapping.expected_responder().context() != target.context() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "target and responder contexts differ".into(),
        ));
    }
    if mapping.expected_responder().value().is_none() {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "EA189 bindings require an expected responder".into(),
        ));
    }
    if mapping.role().role() != &EcuRole::Engine {
        return Err(Ea189ProfileError::InvalidTargetEvidence(
            "initial EA189 bindings are limited to the engine role".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ea189ProfileError {
    UnsupportedSemantic(String),
    NotGenericSignal(String),
    InvalidTargetEvidence(String),
    MissingHardwareValidation,
    DuplicateSemantic(String),
}

impl std::fmt::Display for Ea189ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSemantic(semantic) => {
                write!(
                    formatter,
                    "EA189 profile rejected unsupported semantic: {semantic}"
                )
            }
            Self::NotGenericSignal(semantic) => {
                write!(
                    formatter,
                    "EA189 profile requires a generic signal: {semantic}"
                )
            }
            Self::InvalidTargetEvidence(reason) => {
                write!(
                    formatter,
                    "EA189 profile rejected target evidence: {reason}"
                )
            }
            Self::MissingHardwareValidation => {
                formatter.write_str("EA189 profile evidence requires hardware validation")
            }
            Self::DuplicateSemantic(semantic) => {
                write!(
                    formatter,
                    "EA189 profile already binds semantic: {semantic}"
                )
            }
        }
    }
}

impl std::error::Error for Ea189ProfileError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{
        ProtocolContext, RequestAddress, RequestTarget, RequestTargetEvidence, ResponderIdentity,
        RoleAssignment,
    };

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn mapping() -> EcuTargetMapping {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Physical);
        EcuTargetMapping::new(
            RoleAssignment::new(EcuRole::Engine, provenance("validated topology")),
            RequestTargetEvidence::new(
                RequestTarget::concrete(context.clone(), RequestAddress::new("elm-header", "7E0")),
                provenance("validated topology"),
            ),
            ResponderIdentity::address(context, "7E8"),
        )
    }

    fn evidence() -> ProfileEvidence {
        ProfileEvidence::new(provenance("sanitized hardware fixture"), "fixture-01").unwrap()
    }

    #[test]
    fn experimental_decoder_uses_unsigned_calculated_soot() {
        let reading = decode_experimental_dpf(
            Ea189DpfProbe::SootMassCalculated,
            &[0x62, 0x11, 0x4f, 0x04, 0xf8],
        )
        .unwrap();

        assert_eq!(reading.semantic(), "dpf.soot_mass_calculated");
        assert_eq!(reading.did(), 0x114f);
        assert_eq!(reading.value(), 12.72);
        assert_eq!(reading.unit(), "g");
        assert!(reading.is_experimental());
    }

    #[test]
    fn experimental_decoder_preserves_negative_measured_soot() {
        let reading = decode_experimental_dpf(
            Ea189DpfProbe::SootMassMeasured,
            &[0x62, 0x11, 0x4e, 0xfe, 0xfc],
        )
        .unwrap();

        assert_eq!(reading.value(), -2.6);
        assert_eq!(reading.unit(), "g");
        assert!(reading.decoder().contains("i16"));
    }

    #[test]
    fn experimental_decoder_applies_temperature_hypothesis_to_both_candidates() {
        for probe in [
            Ea189DpfProbe::PreDpfTemperature,
            Ea189DpfProbe::PostDpfTemperature,
        ] {
            let reading = decode_experimental_dpf(
                probe,
                &[0x62, (probe.id() >> 8) as u8, probe.id() as u8, 0x0b, 0xb9],
            )
            .unwrap();
            assert_eq!(reading.value(), 27.0);
            assert_eq!(reading.unit(), "°C");
            assert!(reading.is_experimental());
        }
    }

    #[test]
    fn experimental_decoder_rejects_undecoded_gate_a_candidates() {
        for probe in [
            Ea189DpfProbe::DistanceSinceRegeneration,
            Ea189DpfProbe::TimeSinceRegeneration,
            Ea189DpfProbe::DifferentialPressure,
        ] {
            assert_eq!(
                decode_experimental_dpf(
                    probe,
                    &[0x62, (probe.id() >> 8) as u8, probe.id() as u8, 0x00, 0x01],
                ),
                Err(Ea189DpfDecodeError::UnsupportedProbe(probe))
            );
        }
    }

    #[test]
    fn experimental_decoder_requires_exact_positive_two_byte_payload() {
        let probe = Ea189DpfProbe::SootMassCalculated;
        for response in [
            vec![],
            vec![0x62],
            vec![0x62, 0x11],
            vec![0x62, 0x11, 0x4f],
            vec![0x62, 0x11, 0x4f, 0x00],
            vec![0x62, 0x11, 0x4f, 0x00, 0x01, 0x00],
            vec![0x61, 0x11, 0x4f, 0x00, 0x01],
        ] {
            assert_eq!(
                decode_experimental_dpf(probe, &response),
                Err(Ea189DpfDecodeError::MalformedPositiveResponse)
            );
        }
    }

    #[test]
    fn experimental_decoder_rejects_wrong_did() {
        assert_eq!(
            decode_experimental_dpf(
                Ea189DpfProbe::SootMassCalculated,
                &[0x62, 0x11, 0x4e, 0x00, 0x01],
            ),
            Err(Ea189DpfDecodeError::WrongDid {
                expected: 0x114f,
                actual: 0x114e,
            })
        );
    }

    #[test]
    fn identity_is_distinct_from_generic_profile_without_vehicle_identity() {
        let profile = Ea189Profile::new();
        assert_eq!(profile.identity().id(), PROFILE_ID);
        assert_eq!(profile.identity().engine_family(), "EA189");
        assert_ne!(profile.identity().id(), "obd2-v1");
        assert!(profile.signals().next().is_none());
        assert!(!format!("{profile:?}").contains("VIN"));
    }

    #[test]
    fn gate_a_dpf_probe_is_exactly_seven_candidate_reads() {
        let expected = [
            ("dpf.soot_mass_calculated", 0x114f, [0x22, 0x11, 0x4f]),
            ("dpf.soot_mass_measured", 0x114e, [0x22, 0x11, 0x4e]),
            (
                "dpf.distance_since_regeneration",
                0x1156,
                [0x22, 0x11, 0x56],
            ),
            ("dpf.time_since_regeneration", 0x115e, [0x22, 0x11, 0x5e]),
            ("exhaust.temperature.pre_dpf", 0x11b2, [0x22, 0x11, 0xb2]),
            ("exhaust.temperature.post_dpf", 0x10f9, [0x22, 0x10, 0xf9]),
            ("dpf.differential_pressure", 0x14f5, [0x22, 0x14, 0xf5]),
        ];
        assert_eq!(EA189_DPF_DIDS.len(), expected.len());
        assert_eq!(Ea189DpfProbe::ALL.len(), expected.len());
        for ((definition, probe), expected) in
            EA189_DPF_DIDS.iter().zip(Ea189DpfProbe::ALL).zip(expected)
        {
            assert_eq!(
                (
                    definition.semantic(),
                    definition.id(),
                    definition.request_bytes()
                ),
                expected
            );
            assert_eq!(probe.definition(), *definition);
        }
    }

    #[test]
    fn dpf_probe_request_requires_an_engine_known_ecu() {
        let target = KnownTarget::new("validated-engine").unwrap();
        let request =
            Ea189DpfProbeRequest::for_engine(Ea189DpfProbe::SootMassMeasured, target.clone());
        assert_eq!(request.target().as_str(), "validated-engine");
        assert!(Ea189DpfProbeRequest::from_scope(
            Ea189DpfProbe::SootMassMeasured,
            DiagnosticScope::vehicle_wide(),
        )
        .is_err());
        assert!(Ea189DpfProbeRequest::from_scope(
            Ea189DpfProbe::SootMassMeasured,
            DiagnosticScope::known_ecu(JobEcuRole::Unknown, target,),
        )
        .is_err());
    }

    #[test]
    fn binding_preserves_generic_request_and_exposes_evidence() {
        let mut profile = Ea189Profile::new();
        profile
            .bind_generic_signal("engine.rpm", mapping(), evidence())
            .unwrap();
        let binding = profile.binding("engine.rpm").unwrap();

        assert_eq!(binding.request().bytes(), [0x01, 0x0c]);
        assert_eq!(binding.metadata().profile, "obd2-v1");
        assert_eq!(
            binding
                .target_mapping()
                .target()
                .target()
                .address()
                .unwrap()
                .value(),
            "7E0"
        );
        assert_eq!(binding.evidence().confidence(), Confidence::High);
        assert_eq!(binding.evidence().hardware_validation(), "fixture-01");
    }

    #[test]
    fn no_evidence_means_no_profile_signal_and_closed_operations_stay_rejected() {
        let mut profile = Ea189Profile::new();
        assert!(matches!(
            profile.bind_generic_signal("dtc.clear", mapping(), evidence(),),
            Err(Ea189ProfileError::UnsupportedSemantic(_))
        ));
        assert!(profile.binding("dtc.clear").is_none());
        assert!(crate::prepare_read("dtc.clear").is_err());
    }

    #[test]
    fn speculative_or_wrong_target_facts_are_not_admitted() {
        let context = ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional);
        let invalid = EcuTargetMapping::new(
            RoleAssignment::new(EcuRole::Engine, provenance("hypothesis")),
            RequestTargetEvidence::new(
                RequestTarget::functional(context.clone()),
                provenance("hypothesis"),
            ),
            ResponderIdentity::opaque(context, "7E8"),
        );
        let mut profile = Ea189Profile::new();
        assert!(matches!(
            profile.bind_generic_signal("engine.rpm", invalid, evidence()),
            Err(Ea189ProfileError::InvalidTargetEvidence(_))
        ));
        assert!(profile.signals().next().is_none());
    }
}
