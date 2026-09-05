# Topology Provider evidence contract

Topology Providers are the domain seam for adding reviewed manufacturer/platform **configured or installed ECU evidence** to OBDentic without turning discovery into address scanning or a raw protocol surface.

This document describes the transport-free contract introduced by issue #127 and its private vehicle-cache persistence added by issue #130, both implementation slices of #125.

## Evidence meanings remain separate

A Topology Provider contributes only installation/topology evidence:

```text
Topology Provider
  -> configured / installed controller evidence

functional or targeted diagnostic response
  -> observed / reachable evidence

Vehicle Knowledge
  -> logical ECU role / semantic applicability

independently reviewed addressing evidence
  -> request target
```

These facts are deliberately not interchangeable.

A controller listed by a provider is not automatically reachable. A responder observed through functional OBD is not automatically proof that the controller is configured in a manufacturer installation list. A manufacturer logical address is not a CAN request ID or another concrete transport target unless a separately reviewed mapping establishes that relationship.

## Provider result

A provider result carries:

- stable provider ID and version;
- applicability scope plus provenance;
- explicit provider status;
- explicit provider coverage state;
- zero or more configured/installed ECU entries;
- evidence references;
- an optional request-target mapping only when that mapping is supplied as separate `RequestTargetEvidence`.

The provider entry type has no fields for responder observations, reachability or ECU role. That prevents installation evidence from silently acquiring stronger meaning during conversion into the shared `EcuTopology` model.

## Status and coverage

Provider status distinguishes:

- `Completed`;
- `Unavailable` in the current context;
- `Blocked` by an evidence/safety gate;
- `NotApplicable`;
- `Failed` during provider execution.

Coverage distinguishes:

- `Unknown`;
- `Partial`;
- `Complete` for the provider's declared scope;
- `NotApplicable`.

The domain constructor rejects inconsistent combinations. In particular, blocked or unavailable providers cannot claim complete coverage and cannot return installed ECU entries. A failed provider can preserve partial entries only when coverage is explicitly partial.

A `Complete` provider result is scoped to that provider's declared applicability. The merge layer does not combine several unrelated provider scopes into a guessed global “complete vehicle” claim.

## Deterministic merge

`merge_topology_provider_results` combines provider evidence with the existing functional/observed `EcuTopology`.

The rules are intentionally conservative:

1. provider results and entries are normalized into deterministic order;
2. exact duplicate entries inside one provider result are deduplicated;
3. independent facts from separate providers retain separate provenance;
4. functional responders absent from a provider result remain preserved;
5. configured provider entries do not gain responders, reachability or roles;
6. address-looking logical identifiers are never converted into request targets;
7. an explicit request target is retained only when it arrived as independent `RequestTargetEvidence`.

No value plausibility or numeric-address resemblance participates in the merge.

## Coverage reporting

The merged inventory keeps structured provider records rather than returning a single misleading completeness boolean.

A coarse coverage class is available for presentation:

```text
FunctionalObdOnly
TopologyProviderEvidenceAvailable
TopologyProviderUnavailableOrBlocked
```

The full provider records remain available so consumers can show which provider completed, was partial, was blocked, failed, or did not apply.

An empty blocked/failed provider result therefore never means “the vehicle has no ECUs”. It means that the manufacturer/platform installation source did not produce authoritative inventory evidence in that run.

## Private vehicle-cache persistence

Topology-provider output is **vehicle-instance evidence**, not canonical Vehicle Knowledge. It is persisted in the same local private `VehicleCacheSnapshot` that already stores observed topology, capabilities, request-target mappings and bounded ECU-identification observations.

Vehicle-cache schema v5 stores complete typed `TopologyProviderResult` values, including:

- provider ID/version;
- applicability scope and provenance;
- provider status and coverage;
- provider-level evidence references;
- configured-controller identity/logical-address evidence and provenance;
- entry-level evidence references;
- optional `RequestTargetEvidence` only when it was already supplied independently.

The cache normalizes provider-result ordering deterministically. Persisting a configured controller does **not** create a responder observation, a reachability fact or an ECU role. A logical address remains a logical address after reload; the decoder never promotes it into a request target.

### Cache compatibility

New cache writes use:

```text
OBDENTIC-VEHICLE-CACHE  5
```

The parser continues to read v1, v2, v3 and v4. Older cache records deserialize with an empty topology-provider result collection. They are not reinterpreted as having complete or absent manufacturer topology evidence.

Provider status/coverage is reconstructed through the same domain constructor used by live/in-memory results, so inconsistent combinations fail closed while loading.

### Privacy and validation

Provider metadata passes through the existing private-cache validation rules. Raw VIN-like identifiers are rejected from provider IDs, applicability fields, provenance, evidence references, configured identifiers/logical addresses and request-target metadata.

Provider entry/reference counts are bounded during parsing. Cache files remain local vehicle evidence and must not be committed to the repository.

### Validation signature remains observation-based

Topology-provider installation evidence is deliberately excluded from `VehicleCacheSnapshot::validation_signature()`.

That signature remains based on the bounded functional OBD evidence already used for cache revalidation. A persisted manufacturer installation list therefore cannot silently become proof that an ECU is currently reachable or that a cached request target is live-valid.

## Current EA189 / PQ35 state

The existing research in `docs/research/vw-gateway-installation-list.md` and issue #35 remains a negative safety result for the owned EA189/PQ35 context.

OBDentic currently does **not** have sufficiently evidenced exact gateway request, address mapping and session semantics to execute a VW installation-list provider safely. The domain/cache model can represent that provider as `Blocked` with unknown coverage and persist that fact across restarts, but it adds no VW gateway traffic.

A future live VW provider requires new reviewed evidence that resolves the #35 gaps. It must not be implemented by blind address scanning, DID scanning, undocumented session escalation or copying third-party request sequences without provenance.

## Transport boundary

The provider domain and cache persistence contain no provider execution trait, adapter handle, `DiagnosticSession`, BLE/ELM/CAN/UDS call or raw request field.

Future orchestration may execute a reviewed provider and then persist the resulting domain evidence, but transport remains below the typed diagnostic/safety boundary. The provider result itself is evidence only.

## Discovery follow-up

With the provider domain and persistence seam established, the remaining #125 integration is discovery orchestration: applicable reviewed providers can later be composed with functional OBD discovery while preserving fallback and structured partial-coverage reporting.

That orchestration remains a separate slice. It must reuse the existing private cache and topology types, and it must not add live VW/PQ35 gateway traffic until new reviewed evidence resolves issue #35.
