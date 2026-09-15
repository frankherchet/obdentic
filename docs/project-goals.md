# OBDentic project goals and product contract

This document defines the long-term product goals and the architecture constraints that implementation issues and pull requests should preserve.

GitHub milestones and issues describe the implementation order. This document describes what OBDentic is intended to become.

## Product goal

OBDentic is a local, transparent and AI-assisted vehicle-diagnostics platform intended to grow across vehicle brands, ECU families and Bluetooth diagnostic adapters.

The system should let a human or an AI agent understand a vehicle through semantic facts, captures and reproducible evidence without exposing an unrestricted CAN, UDS or ELM command console.

The target information flow is:

```text
vehicle
  -> adapter backend
  -> protocol normalization
  -> observed vehicle / ECU inventory
  + pinned canonical Vehicle Knowledge
  -> effective Vehicle Knowledge
  -> semantic reads / diagnostic jobs / recording profiles
  -> captures / snapshots
  -> diagnostic reasoning
  -> presentation
```

The dependency direction must remain:

```text
transport evidence
  -> protocol normalization
  -> vehicle facts
  -> semantic snapshots
  -> diagnostic reasoning
  -> presentation
```

Never reverse that direction by letting presentation, diagnosis or an AI agent invent protocol traffic.

## 1. Multi-brand vehicle diagnostics

OBDentic is not a VW- or EA189-specific product.

The VW EA189 is the first deep vehicle use case and an evidence source for proving the architecture. New manufacturers and vehicle families must reuse the same generic adapter, protocol, inventory, knowledge, scheduling, capture and diagnostic layers rather than introducing parallel stacks.

Vehicle-specific knowledge belongs in versioned, provenance-aware canonical knowledge and must remain separate from private observations about one concrete vehicle.

A new vehicle platform should be onboarded incrementally:

1. collect bounded diagnostic evidence;
2. preserve responder and topology evidence;
3. identify ECUs using standards-based or manufacturer-specific reviewed read-only discovery where available;
4. build private observed inventory;
5. resolve canonical knowledge conservatively;
6. expose semantic facts;
7. validate with deterministic offline fixtures;
8. validate separately on real hardware.

Unknown OEM identifiers must never be discovered by blind DID or address scanning.

## 2. Automatic ECU inventory and installation list

OBDentic should automatically derive the most complete ECU installation inventory that can be obtained safely and reproducibly for the connected vehicle.

The desired result is a vehicle-local inventory conceptually shaped as:

```text
VehicleInstance
  -> EcuInstance[]
     -> configured / installed evidence
     -> observed / reachable evidence
     -> logical role + provenance
     -> request target + provenance
     -> capabilities
     -> identification / fingerprint evidence
```

These facts must remain distinct:

```text
configured / installed
  != observed / reachable
  != logical role
  != diagnostic request target
```

A manufacturer installation list may establish that an ECU is configured but must not by itself prove that the ECU is currently reachable. A functional OBD responder may prove reachability but must not by itself prove the ECU role or complete vehicle installation list.

### Discovery strategy

OBDentic should combine, where evidence allows:

- standards-based functional responder discovery;
- bounded standards-based ECU identification for already evidenced targets;
- manufacturer/platform-specific read-only topology providers, such as an installation-list mechanism, when the request and addressing semantics are reviewed and evidenced;
- previously validated private inventory/cache evidence with bounded revalidation.

The objective is to make the installation list as automatic as practical without using blind CAN-address sweeps, arbitrary OEM DID scanning or plausibility-based guessing.

If a platform has no trustworthy installation-list mechanism, OBDentic must report the inventory as partial rather than pretending functional OBD visibility is the complete vehicle topology.

## 3. Modular Bluetooth adapter support

Bluetooth is the current transport scope of the project.

OBDentic should support multiple Bluetooth diagnostic adapters without coupling Vehicle Knowledge, scheduling, capture or diagnosis to one device.

The intended layering is:

```text
Bluetooth AdapterBackend
        -> shared or adapter-specific command dialect
        -> diagnostic protocol knowledge
        -> normalized responder / transport evidence
```

For the existing Carly adapter the contract is:

```text
CarlyCuaV200 AdapterBackend
        +
shared ElmDialect
```

not:

```text
Carly == ELM327
```

Adapter-specific responsibilities include discovery, recognition, BLE services/characteristics, connection behavior, identity checks and hardware quirks. Reusable ELM-compatible ASCII behavior belongs in a shared dialect layer.

Adding another Bluetooth adapter must not require duplicating semantic decoders or modifying vehicle-specific knowledge.

USB and Wi-Fi transports are not current product goals. The design should not needlessly prevent them later, but implementation work should remain focused on Bluetooth until this product contract is deliberately changed.

## 4. Recording profiles and long-running evidence capture

OBDentic should support declarative recording profiles for longer observations of vehicle state.

Profiles describe observation intent only:

```yaml
version: 1
id: example-drive
observations:
  - semantic: engine.rpm
    interval: 1s
  - semantic: vehicle.speed
    interval: 2s
```

Profiles may select semantic facts, requested timing and narrowly defined observation policy. They must not contain:

- raw PID or DID definitions;
- CAN addresses;
- raw UDS payloads or service IDs;
- ELM commands;
- decoder formulas;
- SecurityAccess/session-control instructions;
- coding/adaptation/actuator commands;
- mutation capability switches.

Profiles resolve against effective Vehicle Knowledge and pass through Subscription Policy and Safety Policy before transport.

Existing and future capture backends consume the same format-independent evidence model. Recording must preserve enough request, response, responder, timing and knowledge provenance to support offline re-decode and diagnosis.

## 5. Diagnostic trouble codes

OBDentic should support reading DTCs through bounded semantic diagnostic jobs.

DTC evidence should remain ECU/responder scoped and preserve original diagnostic responses. DTC presence is a fact, not by itself a component-health verdict.

Coverage should grow from standardized OBD-II DTCs toward evidence-backed manufacturer/ECU-specific DTC access without introducing a raw protocol-send API.

## 6. Safety model: read-only by default, narrowly gated service mutation

The default OBDentic capability is read-only.

Ordinary CLI use, capture, TUI, MCP and AI-assisted diagnosis must remain unable to mutate the vehicle unless the process was explicitly started in a separately reviewed service capability mode.

### Initial allowed mutation goal

The first and currently only intended mutating service capability is DTC clearing.

DTC clearing must satisfy all of the following:

- unavailable in the default mode;
- enabled only explicitly at process start through a narrow capability;
- the capability cannot be enabled dynamically by a profile, layout, MCP call, AI prompt or knowledge file;
- exposed only as a typed, reviewed semantic diagnostic job;
- no caller-provided raw service byte, CAN address, UDS payload or ELM command;
- target selection must be known/evidenced or otherwise explicitly bounded by the reviewed job design;
- original DTC/read evidence should be preserved before clearing and the result should be observable afterwards where practical;
- capture/audit must make the mutating operation explicit;
- enabling DTC clear must not implicitly enable any other mutating operation.

Conceptually:

```text
default process
  capability = ReadOnly
  -> reads and bounded read-only diagnostic jobs only

explicit service process
  capability = ReadOnly + DtcClear
  -> same read operations
  -> one closed DTC-clear job
```

This must never become a generic `--write` or `unsafe raw command` mode.

### Operations that remain outside the product goal

Unless this project contract is explicitly revised in the future, the following remain unavailable:

- arbitrary CAN frame injection;
- arbitrary UDS or OBD service execution;
- arbitrary ELM command execution from higher layers;
- SecurityAccess;
- coding;
- adaptation / basic settings;
- actuator / output tests;
- ECU reset;
- forced regeneration;
- arbitrary RoutineControl;
- generic memory/identifier writes.

The typed core, not UI confirmation or prompt instructions, remains the safety boundary.

## 7. AI-assisted diagnostics

AI is a diagnostic consumer and reasoning layer, not a protocol executor.

An AI agent should be able to:

- inspect vehicle and ECU inventory;
- list effective semantic facts;
- request approved semantic reads and read-only diagnostic jobs;
- request recording profiles/observation demand;
- inspect captures and snapshots offline;
- correlate facts and explain hypotheses;
- propose useful follow-up evidence collection;
- control presentation/layout through semantic interfaces.

The AI-facing surface must not expose arbitrary raw CAN/UDS/ELM commands and must not be able to elevate the process from read-only mode into the DTC-clear service capability.

When a mutating service mode is eventually active, AI-assisted operation still requires a separately reviewed interface policy; the existence of the process capability alone does not imply that MCP/agent tooling automatically receives that operation.

## 8. Evidence, provenance and reproducibility

OBDentic should make diagnosis explainable and repeatable.

The canonical capture flow remains:

```text
vehicle
  -> raw diagnostic/responder evidence
  -> deterministic protocol normalization
  -> deterministic vehicle facts where justified
  -> persisted capture

capture
  -> offline re-decode
  -> correlation
  -> diagnostic interpretation
```

Responder evidence must be preserved completely. Never select a response because its decoded value looks most plausible.

Vehicle-specific knowledge must preserve provenance, confidence and hardware-validation state. Research remains classified explicitly, including VERIFIED, COMMUNITY, INFERRED and EXPERIMENTAL where those categories apply.

Canonical reusable knowledge and private observed inventory remain separate. VINs, ECU serial numbers and raw vehicle captures must not be silently promoted into the public knowledge repository.

## 9. Architectural invariants

The following remain core invariants across all product goals:

- one process/session actor exclusively owns one physical Bluetooth adapter;
- physical diagnostic commands remain sequential unless a future reviewed protocol/backend proves otherwise;
- adapter hardware identity is separate from command dialect;
- protocol knowledge is separate from Vehicle Knowledge;
- Vehicle Knowledge translates normalized evidence into deterministic facts;
- diagnostic reasoning translates facts into hypotheses/explanations;
- layouts and profiles never become raw protocol command surfaces;
- capture is passive and cannot initiate vehicle traffic;
- original evidence is authoritative and remains available for offline re-analysis;
- ambiguity remains explicit rather than guessed away;
- hardware acceptance is separate from offline/unit acceptance;
- new vehicle knowledge requires reviewable evidence and provenance.

## 10. Definition of success

OBDentic reaches the intended product direction when the same generic system can, for multiple brands and Bluetooth adapters:

1. recognize/connect through an appropriate adapter backend;
2. automatically derive as complete an ECU installation inventory as safely supported by the platform;
3. resolve reusable canonical Vehicle Knowledge from observed ECU identity/capability evidence;
4. expose semantic vehicle facts without requiring users or agents to know protocol addresses or payloads;
5. read DTCs and record long-running semantic observations;
6. preserve enough evidence for deterministic offline replay/re-decode;
7. support AI-assisted diagnostic reasoning over facts and captures;
8. remain read-only by default;
9. permit DTC clearing only through an explicit, narrow, process-start service capability;
10. keep arbitrary mutation and raw diagnostic command injection structurally unavailable.

When an implementation choice conflicts with these goals, update this document deliberately as part of the architectural decision rather than allowing the project contract to drift implicitly.
