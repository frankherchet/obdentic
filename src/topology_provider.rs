//! Transport-free topology-provider evidence and deterministic composition.
//!
//! Provider results describe configured/installed controller evidence and
//! coverage. They cannot issue transport requests or infer reachability,
//! logical roles, or request targets from address-looking values.

use std::fmt;

use crate::topology::{
    AddressingContext, ConfiguredController, EcuNode, EcuTopology, Protocol, ProtocolContext,
    Provenance, RequestTargetEvidence,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyProviderError {
    EmptyProviderName,
    ZeroProviderVersion,
    EmptyManufacturer,
    EmptyPlatform,
    EmptyEvidenceReference,
    InvalidStatusCoverage,
    NonApplicableProviderHasEntries,
    BlockedOrUnavailableProviderHasEntries,
}

impl fmt::Display for TopologyProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyProviderName => "topology provider name must not be empty",
            Self::ZeroProviderVersion => "topology provider version must be greater than zero",
            Self::EmptyManufacturer => "topology provider manufacturer must not be empty",
            Self::EmptyPlatform => "topology provider platform must not be empty",
            Self::EmptyEvidenceReference => {
                "topology provider evidence reference must not be empty"
            }
            Self::InvalidStatusCoverage => "topology provider status and coverage are inconsistent",
            Self::NonApplicableProviderHasEntries => {
                "a not-applicable topology provider must not contain installed ECU entries"
            }
            Self::BlockedOrUnavailableProviderHasEntries => {
                "a blocked or unavailable topology provider must not contain installed ECU entries"
            }
        })
    }
}

impl std::error::Error for TopologyProviderError {}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyProviderId {
    name: String,
    version: u32,
}

impl TopologyProviderId {
    pub fn new(name: impl Into<String>, version: u32) -> Result<Self, TopologyProviderError> {
        let name = normalize_non_empty(name.into(), TopologyProviderError::EmptyProviderName)?;
        if version == 0 {
            return Err(TopologyProviderError::ZeroProviderVersion);
        }
        Ok(Self { name, version })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyProviderScope {
    manufacturer: Option<String>,
    platform: Option<String>,
}

impl TopologyProviderScope {
    pub fn generic() -> Self {
        Self {
            manufacturer: None,
            platform: None,
        }
    }

    pub fn manufacturer(manufacturer: impl Into<String>) -> Result<Self, TopologyProviderError> {
        Ok(Self {
            manufacturer: Some(normalize_non_empty(
                manufacturer.into(),
                TopologyProviderError::EmptyManufacturer,
            )?),
            platform: None,
        })
    }

    pub fn platform(
        manufacturer: impl Into<String>,
        platform: impl Into<String>,
    ) -> Result<Self, TopologyProviderError> {
        let manufacturer = normalize_non_empty(
            manufacturer.into(),
            TopologyProviderError::EmptyManufacturer,
        )?;
        let platform = normalize_non_empty(platform.into(), TopologyProviderError::EmptyPlatform)?;
        Ok(Self {
            manufacturer: Some(manufacturer),
            platform: Some(platform),
        })
    }

    pub fn manufacturer_name(&self) -> Option<&str> {
        self.manufacturer.as_deref()
    }

    pub fn platform_name(&self) -> Option<&str> {
        self.platform.as_deref()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyProviderApplicability {
    scope: TopologyProviderScope,
    provenance: Provenance,
}

impl TopologyProviderApplicability {
    pub fn new(scope: TopologyProviderScope, provenance: Provenance) -> Self {
        Self { scope, provenance }
    }

    pub fn scope(&self) -> &TopologyProviderScope {
        &self.scope
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct EvidenceReference(String);

impl EvidenceReference {
    pub fn new(reference: impl Into<String>) -> Result<Self, TopologyProviderError> {
        Ok(Self(normalize_non_empty(
            reference.into(),
            TopologyProviderError::EmptyEvidenceReference,
        )?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum TopologyProviderStatus {
    Completed,
    Unavailable,
    Blocked,
    NotApplicable,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum TopologyProviderCoverage {
    Unknown,
    Partial,
    Complete,
    NotApplicable,
}

/// One configured/installed-controller fact produced by a topology provider.
///
/// There are deliberately no responder, reachability, or role fields. A
/// request target can only arrive as separately sourced `RequestTargetEvidence`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct InstalledEcuEvidence {
    context: ProtocolContext,
    configured: ConfiguredController,
    request_target: Option<RequestTargetEvidence>,
    evidence_references: Vec<EvidenceReference>,
}

impl InstalledEcuEvidence {
    pub fn new(context: ProtocolContext, configured: ConfiguredController) -> Self {
        Self {
            context,
            configured,
            request_target: None,
            evidence_references: Vec::new(),
        }
    }

    pub fn with_request_target(mut self, request_target: RequestTargetEvidence) -> Self {
        self.request_target = Some(request_target);
        self
    }

    pub fn with_evidence_reference(mut self, reference: EvidenceReference) -> Self {
        self.evidence_references.push(reference);
        self.evidence_references.sort();
        self.evidence_references.dedup();
        self
    }

    pub fn context(&self) -> &ProtocolContext {
        &self.context
    }

    pub fn configured_controller(&self) -> &ConfiguredController {
        &self.configured
    }

    pub fn request_target(&self) -> Option<&RequestTargetEvidence> {
        self.request_target.as_ref()
    }

    pub fn evidence_references(&self) -> &[EvidenceReference] {
        &self.evidence_references
    }

    pub fn to_topology_node(&self) -> EcuNode {
        let mut node = EcuNode::new(self.context.clone(), self.configured.provenance().clone())
            .with_configured_controller(self.configured.clone());
        if let Some(request_target) = &self.request_target {
            node = node.with_request_target(request_target.clone());
        }
        node
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyProviderResult {
    id: TopologyProviderId,
    applicability: TopologyProviderApplicability,
    status: TopologyProviderStatus,
    coverage: TopologyProviderCoverage,
    entries: Vec<InstalledEcuEvidence>,
    evidence_references: Vec<EvidenceReference>,
}

impl TopologyProviderResult {
    pub fn new(
        id: TopologyProviderId,
        applicability: TopologyProviderApplicability,
        status: TopologyProviderStatus,
        coverage: TopologyProviderCoverage,
        entries: impl IntoIterator<Item = InstalledEcuEvidence>,
        evidence_references: impl IntoIterator<Item = EvidenceReference>,
    ) -> Result<Self, TopologyProviderError> {
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        entries.sort();
        entries.dedup();
        validate_status_coverage(status, coverage, entries.is_empty())?;

        let mut evidence_references = evidence_references.into_iter().collect::<Vec<_>>();
        evidence_references.sort();
        evidence_references.dedup();

        Ok(Self {
            id,
            applicability,
            status,
            coverage,
            entries,
            evidence_references,
        })
    }

    pub fn id(&self) -> &TopologyProviderId {
        &self.id
    }

    pub fn applicability(&self) -> &TopologyProviderApplicability {
        &self.applicability
    }

    pub const fn status(&self) -> TopologyProviderStatus {
        self.status
    }

    pub const fn coverage(&self) -> TopologyProviderCoverage {
        self.coverage
    }

    pub fn entries(&self) -> &[InstalledEcuEvidence] {
        &self.entries
    }

    pub fn evidence_references(&self) -> &[EvidenceReference] {
        &self.evidence_references
    }

    pub fn to_topology(&self) -> EcuTopology {
        EcuTopology::from_nodes(
            self.entries
                .iter()
                .map(InstalledEcuEvidence::to_topology_node),
        )
    }

    fn coverage_record(&self) -> TopologyProviderCoverageRecord {
        TopologyProviderCoverageRecord {
            id: self.id.clone(),
            applicability: self.applicability.clone(),
            status: self.status,
            coverage: self.coverage,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyProviderCoverageRecord {
    id: TopologyProviderId,
    applicability: TopologyProviderApplicability,
    status: TopologyProviderStatus,
    coverage: TopologyProviderCoverage,
}

impl TopologyProviderCoverageRecord {
    pub fn id(&self) -> &TopologyProviderId {
        &self.id
    }

    pub fn applicability(&self) -> &TopologyProviderApplicability {
        &self.applicability
    }

    pub const fn status(&self) -> TopologyProviderStatus {
        self.status
    }

    pub const fn coverage(&self) -> TopologyProviderCoverage {
        self.coverage
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum TopologyInventoryCoverageClass {
    FunctionalObdOnly,
    TopologyProviderEvidenceAvailable,
    TopologyProviderUnavailableOrBlocked,
}

/// Coverage remains structured so separate provider scopes are never collapsed
/// into a guessed global "complete vehicle" boolean.
#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyInventoryCoverage {
    functional_obd_evidence_present: bool,
    providers: Vec<TopologyProviderCoverageRecord>,
}

impl TopologyInventoryCoverage {
    fn new(
        functional_obd_evidence_present: bool,
        providers: impl IntoIterator<Item = TopologyProviderCoverageRecord>,
    ) -> Self {
        let mut providers = providers.into_iter().collect::<Vec<_>>();
        providers.sort();
        providers.dedup();
        Self {
            functional_obd_evidence_present,
            providers,
        }
    }

    pub const fn functional_obd_evidence_present(&self) -> bool {
        self.functional_obd_evidence_present
    }

    pub fn providers(&self) -> &[TopologyProviderCoverageRecord] {
        &self.providers
    }

    pub fn class(&self) -> TopologyInventoryCoverageClass {
        if self.providers.iter().any(|record| {
            matches!(
                record.coverage(),
                TopologyProviderCoverage::Partial | TopologyProviderCoverage::Complete
            )
        }) {
            TopologyInventoryCoverageClass::TopologyProviderEvidenceAvailable
        } else if self.providers.iter().any(|record| {
            matches!(
                record.status(),
                TopologyProviderStatus::Blocked
                    | TopologyProviderStatus::Unavailable
                    | TopologyProviderStatus::Failed
            )
        }) {
            TopologyInventoryCoverageClass::TopologyProviderUnavailableOrBlocked
        } else {
            TopologyInventoryCoverageClass::FunctionalObdOnly
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct TopologyInventory {
    topology: EcuTopology,
    coverage: TopologyInventoryCoverage,
}

impl TopologyInventory {
    pub fn topology(&self) -> &EcuTopology {
        &self.topology
    }

    pub fn coverage(&self) -> &TopologyInventoryCoverage {
        &self.coverage
    }
}

/// Merge provider evidence with the existing functional OBD topology.
///
/// This performs no I/O and never infers links between provider entries and
/// observed responders.
pub fn merge_topology_provider_results(
    functional_topology: &EcuTopology,
    provider_results: &[TopologyProviderResult],
) -> TopologyInventory {
    let mut provider_results = provider_results.to_vec();
    provider_results.sort();
    provider_results.dedup();

    let mut topology = functional_topology.clone();
    for result in &provider_results {
        topology = topology.merge(result.to_topology());
    }

    let functional_obd_evidence_present = functional_topology.nodes().iter().any(|node| {
        matches!(node.context().protocol(), Protocol::Obd2)
            && matches!(node.context().addressing(), AddressingContext::Functional)
            && !node.observed_responders().is_empty()
    });
    let coverage = TopologyInventoryCoverage::new(
        functional_obd_evidence_present,
        provider_results
            .iter()
            .map(TopologyProviderResult::coverage_record),
    );

    TopologyInventory { topology, coverage }
}

fn normalize_non_empty(
    value: String,
    error: TopologyProviderError,
) -> Result<String, TopologyProviderError> {
    let value = value.trim();
    if value.is_empty() {
        Err(error)
    } else {
        Ok(value.to_owned())
    }
}

fn validate_status_coverage(
    status: TopologyProviderStatus,
    coverage: TopologyProviderCoverage,
    entries_empty: bool,
) -> Result<(), TopologyProviderError> {
    if matches!(
        status,
        TopologyProviderStatus::Blocked | TopologyProviderStatus::Unavailable
    ) && !entries_empty
    {
        return Err(TopologyProviderError::BlockedOrUnavailableProviderHasEntries);
    }
    if status == TopologyProviderStatus::NotApplicable && !entries_empty {
        return Err(TopologyProviderError::NonApplicableProviderHasEntries);
    }

    let valid = match status {
        TopologyProviderStatus::Completed => matches!(
            coverage,
            TopologyProviderCoverage::Partial | TopologyProviderCoverage::Complete
        ),
        TopologyProviderStatus::Blocked | TopologyProviderStatus::Unavailable => {
            coverage == TopologyProviderCoverage::Unknown
        }
        TopologyProviderStatus::NotApplicable => {
            coverage == TopologyProviderCoverage::NotApplicable
        }
        TopologyProviderStatus::Failed if entries_empty => {
            coverage == TopologyProviderCoverage::Unknown
        }
        TopologyProviderStatus::Failed => coverage == TopologyProviderCoverage::Partial,
    };

    if valid {
        Ok(())
    } else {
        Err(TopologyProviderError::InvalidStatusCoverage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{
        Confidence, ConfiguredIdentity, LogicalAddress, ObservedResponder, RequestAddress,
        RequestTarget, ResponderIdentity,
    };

    fn provenance(source: &str) -> Provenance {
        Provenance::new(source, Confidence::High).unwrap()
    }

    fn configured_context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Uds, AddressingContext::Unknown)
    }

    fn functional_context() -> ProtocolContext {
        ProtocolContext::new(Protocol::Obd2, AddressingContext::Functional)
    }

    fn applicability(source: &str) -> TopologyProviderApplicability {
        TopologyProviderApplicability::new(
            TopologyProviderScope::platform("Synthetic Motors", "Test Platform").unwrap(),
            provenance(source),
        )
    }

    fn installed(source: &str, id: &str, logical: &str) -> InstalledEcuEvidence {
        InstalledEcuEvidence::new(
            configured_context(),
            ConfiguredController::new(
                Some(ConfiguredIdentity::new("synthetic-installation-list", id)),
                Some(LogicalAddress::new("synthetic-logical-address", logical)),
                provenance(source),
            )
            .unwrap(),
        )
    }

    fn provider(
        name: &str,
        entries: impl IntoIterator<Item = InstalledEcuEvidence>,
    ) -> TopologyProviderResult {
        TopologyProviderResult::new(
            TopologyProviderId::new(name, 1).unwrap(),
            applicability("synthetic applicability"),
            TopologyProviderStatus::Completed,
            TopologyProviderCoverage::Partial,
            entries,
            [EvidenceReference::new("synthetic fixture").unwrap()],
        )
        .unwrap()
    }

    fn functional_topology(responder: &str) -> EcuTopology {
        EcuTopology::from_nodes([
            EcuNode::new(functional_context(), provenance("functional OBD"))
                .with_observed_responder(ObservedResponder::new(
                    ResponderIdentity::address(functional_context(), responder),
                    provenance("functional OBD"),
                )),
        ])
    }

    #[test]
    fn configured_entries_do_not_gain_reachability_or_role() {
        let result = provider(
            "synthetic.installation-list",
            [
                installed("provider", "engine", "01"),
                installed("provider", "abs", "03"),
            ],
        );
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[result]);

        assert_eq!(inventory.topology().nodes().len(), 2);
        assert!(inventory.topology().nodes().iter().all(|node| {
            node.configured_controller().is_some()
                && node.observed_responders().is_empty()
                && node.reachability().is_none()
                && node.role().is_none()
        }));
    }

    #[test]
    fn functional_responder_outside_provider_output_is_preserved() {
        let functional = functional_topology("7E8");
        let result = provider(
            "synthetic.installation-list",
            [installed("provider", "abs", "03")],
        );
        let inventory = merge_topology_provider_results(&functional, &[result]);

        assert_eq!(inventory.topology().nodes().len(), 2);
        assert_eq!(
            inventory
                .topology()
                .nodes()
                .iter()
                .filter(|node| !node.observed_responders().is_empty())
                .count(),
            1
        );
        assert!(inventory.coverage().functional_obd_evidence_present());
    }

    #[test]
    fn request_target_requires_separate_explicit_evidence() {
        let entry =
            installed("provider", "engine", "01").with_request_target(RequestTargetEvidence::new(
                RequestTarget::concrete(
                    ProtocolContext::new(Protocol::Uds, AddressingContext::Physical),
                    RequestAddress::new("reviewed-target-space", "target-engine"),
                ),
                provenance("independent target mapping"),
            ));
        let result = provider("synthetic.installation-list", [entry]);
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[result]);
        let node = &inventory.topology().nodes()[0];

        assert_eq!(
            node.configured_controller()
                .unwrap()
                .logical_address()
                .unwrap()
                .value(),
            "01"
        );
        assert_eq!(
            node.request_target()
                .unwrap()
                .target()
                .address()
                .unwrap()
                .value(),
            "target-engine"
        );
        assert_eq!(
            node.request_target().unwrap().provenance().source(),
            "independent target mapping"
        );
        assert!(node.observed_responders().is_empty());
    }

    #[test]
    fn blocked_and_failed_providers_cannot_claim_complete_inventory() {
        let blocked = TopologyProviderResult::new(
            TopologyProviderId::new("synthetic.blocked", 1).unwrap(),
            applicability("safety review"),
            TopologyProviderStatus::Blocked,
            TopologyProviderCoverage::Unknown,
            [],
            [EvidenceReference::new("negative safety gate").unwrap()],
        )
        .unwrap();
        let failed = TopologyProviderResult::new(
            TopologyProviderId::new("synthetic.failed", 1).unwrap(),
            applicability("runtime evidence"),
            TopologyProviderStatus::Failed,
            TopologyProviderCoverage::Unknown,
            [],
            [],
        )
        .unwrap();
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[blocked, failed]);

        assert_eq!(
            inventory.coverage().class(),
            TopologyInventoryCoverageClass::TopologyProviderUnavailableOrBlocked
        );
        assert!(inventory.topology().nodes().is_empty());
        assert_eq!(
            TopologyProviderResult::new(
                TopologyProviderId::new("synthetic.invalid", 1).unwrap(),
                applicability("safety review"),
                TopologyProviderStatus::Blocked,
                TopologyProviderCoverage::Complete,
                [],
                [],
            ),
            Err(TopologyProviderError::InvalidStatusCoverage)
        );
    }

    #[test]
    fn provider_order_is_deterministic() {
        let first = provider(
            "synthetic.first",
            [installed("first provider", "engine", "01")],
        );
        let second = provider(
            "synthetic.second",
            [installed("second provider", "abs", "03")],
        );
        let functional = functional_topology("7E8");

        let left = merge_topology_provider_results(&functional, &[first.clone(), second.clone()]);
        let right = merge_topology_provider_results(&functional, &[second, first]);
        assert_eq!(left, right);
    }

    #[test]
    fn same_provider_duplicates_normalize_but_cross_provider_provenance_remains() {
        let duplicate = installed("provider one", "engine", "01");
        let first = provider("synthetic.first", [duplicate.clone(), duplicate]);
        assert_eq!(first.entries().len(), 1);

        let second = provider(
            "synthetic.second",
            [installed("provider two", "engine", "01")],
        );
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[first, second]);

        assert_eq!(inventory.topology().nodes().len(), 2);
        let sources = inventory
            .topology()
            .nodes()
            .iter()
            .map(|node| node.configured_controller().unwrap().provenance().source())
            .collect::<Vec<_>>();
        assert!(sources.contains(&"provider one"));
        assert!(sources.contains(&"provider two"));
    }

    #[test]
    fn logical_address_never_becomes_request_target() {
        let result = provider(
            "synthetic.installation-list",
            [installed("provider", "engine", "7E0")],
        );
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[result]);
        let node = &inventory.topology().nodes()[0];

        assert_eq!(
            node.configured_controller()
                .unwrap()
                .logical_address()
                .unwrap()
                .value(),
            "7E0"
        );
        assert!(node.request_target().is_none());
    }

    #[test]
    fn zero_providers_is_functional_obd_only() {
        let functional = functional_topology("7E8");
        let inventory = merge_topology_provider_results(&functional, &[]);

        assert_eq!(inventory.topology(), &functional);
        assert_eq!(
            inventory.coverage().class(),
            TopologyInventoryCoverageClass::FunctionalObdOnly
        );
        assert!(inventory.coverage().functional_obd_evidence_present());
        assert!(inventory.coverage().providers().is_empty());
    }

    #[test]
    fn ea189_pq35_gateway_state_can_remain_blocked_without_transport() {
        let blocked = TopologyProviderResult::new(
            TopologyProviderId::new("vw.pq35.gateway-installation-list", 1).unwrap(),
            TopologyProviderApplicability::new(
                TopologyProviderScope::platform("Volkswagen", "PQ35 / EA189").unwrap(),
                provenance("docs/research/vw-gateway-installation-list.md"),
            ),
            TopologyProviderStatus::Blocked,
            TopologyProviderCoverage::Unknown,
            [],
            [EvidenceReference::new("issue #35 negative safety gate").unwrap()],
        )
        .unwrap();
        let inventory = merge_topology_provider_results(&EcuTopology::new(), &[blocked]);

        assert!(inventory.topology().nodes().is_empty());
        assert_eq!(
            inventory.coverage().class(),
            TopologyInventoryCoverageClass::TopologyProviderUnavailableOrBlocked
        );
        assert_eq!(
            inventory.coverage().providers()[0].id().name(),
            "vw.pq35.gateway-installation-list"
        );
    }
}
