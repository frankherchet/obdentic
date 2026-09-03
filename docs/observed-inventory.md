# Private observed vehicle inventory

OBDentic keeps locally observed vehicle/ECU evidence separate from reusable canonical Knowledge.

```text
connected vehicle
      ↓
raw responder / protocol evidence
      ↓
private local VehicleCache
      ↓
VehicleInventory domain projection
      ↓
effective-Knowledge resolver / later consumers

pinned obdentic-knowledge
      ↓
canonical reviewed definitions
```

`VehicleInventory` describes **what this installation observed about one concrete vehicle**. It is not a second Vehicle Knowledge database and it never writes to the `knowledge/` submodule.

## Privacy boundary

The inventory exposes the cache's privacy-safe local vehicle ID, not a raw VIN. The VIN-to-local-ID mapping remains in the existing private identity index.

Historical cache text remains private persistence evidence. The first domain-projection slice exposes only the number of historical records; it deliberately does not copy arbitrary historical text into the resolver-facing object graph.

ECU identification values, including serial-number-like values, remain private observed evidence. Their presence in an inventory does not promote them into public captures or canonical Knowledge.

## Vehicle → ECU instances

The current projection groups evidence independently per observed responder:

```text
VehicleInventory
  local_vehicle_id
  first_seen / last_seen
  ├─ EcuInstance responder A
  │    ├─ responder evidence
  │    ├─ Mode-01 capability evidence
  │    ├─ evidenced target mappings
  │    └─ standard UDS identification observations
  └─ EcuInstance responder B
       └─ ...
```

Responder-less target evidence is preserved separately as `unassigned_targets`; it is never guessed onto an ECU instance.

Input order does not affect the projected inventory.

## Current identity limitation

The responder identity used by this first slice is an **observation grouping key**, not a claim that a transport/header identity is the permanent ECU identity.

A later #87 persistence slice must introduce a stable local `EcuInstanceId` plus explicit current/history semantics. That migration must be versioned and must not infer ECU sameness from a plausible-looking hardware/software value alone.

## Canonical Knowledge boundary

The inventory may retain which pinned Knowledge definition/revision was used to interpret an ECU-identification observation, but canonical reusable definitions remain owned by `frankherchet/obdentic-knowledge`.

No local discovery path automatically edits, commits or promotes data into the Knowledge repository. Promotion remains an explicit review/evidence workflow.

## Relationship to #86

#86 should consume the storage-independent `VehicleInventory`/`EcuInstance` domain seam rather than parsing cache TSV records or depending on cache-format version details.

The intended flow is:

```text
private observed VehicleInventory
        +
pinned canonical Knowledge
        ↓
effective Vehicle Knowledge
        ↓
semantic Profiles
```

The resolver must not treat observed values as canonical definitions merely because they exist in the local inventory.
