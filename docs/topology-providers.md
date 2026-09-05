# Topology Provider evidence contract

Topology Providers are the domain seam for adding reviewed manufacturer/platform **configured or installed ECU evidence** to OBDentic without turning discovery into address scanning or a raw protocol surface.

This document describes the transport-free contract introduced by issue #127, the first implementation slice of #125.

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

## Current EA189 / PQ35 state

The existing research in `docs/research/vw-gateway-installation-list.md` and issue #35 remains a negative safety result for the owned EA189/PQ35 context.

OBDentic currently does **not** have sufficiently evidenced exact gateway request, address mapping and session semantics to execute a VW installation-list provider safely. The new domain contract can represent that provider as `Blocked` with unknown coverage, but it adds no VW gateway traffic.

A future live VW provider requires new reviewed evidence that resolves the #35 gaps. It must not be implemented by blind address scanning, DID scanning, undocumented session escalation or copying third-party request sequences without provenance.

## Transport boundary

This slice contains no provider execution trait, adapter handle, `DiagnosticSession`, BLE/ELM/CAN/UDS call or raw request field.

Future orchestration may execute a reviewed provider and then produce this domain result, but transport remains below the typed diagnostic/safety boundary. The provider result itself is evidence only.

## Persistence and discovery follow-up

The current `VehicleCacheSnapshot::from_topology()` primarily persists observed responder/target information and does not yet retain configured-controller provider facts.

That integration is intentionally a separate follow-up slice of #125. It should extend the existing private inventory/cache and `vehicle discover` orchestration rather than creating a second topology database.
