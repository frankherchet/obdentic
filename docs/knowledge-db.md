# Canonical Knowledge DB integration

OBDentic consumes canonical Vehicle/ECU Knowledge from the separate [`frankherchet/obdentic-knowledge`](https://github.com/frankherchet/obdentic-knowledge) repository.

## Boundary

The repository split is intentional:

```text
local observed vehicle / ECU inventory
              +
pinned canonical obdentic-knowledge
              ↓
effective Vehicle Knowledge
```

`obdentic-knowledge` contains reviewed declarative definitions. The OBDentic core remains the executable safety boundary and owns protocol primitives, response validation, decoder primitives, `SafetyPolicy`, session ownership, capture and replay.

A Knowledge file may select a closed read-only primitive such as:

```yaml
operation:
  type: uds.read_data_by_identifier
  identifier: "0xF189"
```

It cannot create a raw CAN frame, arbitrary UDS payload/service, ELM command, session change, SecurityAccess, coding/adaptation, actuator test, DTC clear or arbitrary RoutineControl path.

Unknown schema fields, operation kinds and decoder kinds fail closed while loading, before any transport is involved.

## Pinning

The canonical repository is checked in as the `knowledge/` Git submodule. `knowledge.lock` records the same immutable revision and schema version in a simple runtime-readable form:

```text
repository = frankherchet/obdentic-knowledge
revision = <40-character commit SHA>
schema_version = 1
```

The Git submodule is the source pin. `knowledge.lock` exists so runtime/capture code can expose deterministic Knowledge provenance without shelling out to Git.

Normal OBDentic runtime must never run `git pull`, fetch GitHub or otherwise update Knowledge implicitly.

## Checkout

Clone with submodules:

```bash
git clone --recurse-submodules https://github.com/frankherchet/obdentic.git
```

or initialize an existing checkout:

```bash
git submodule update --init --recursive
```

If `knowledge/` is absent or empty, the loader fails explicitly. It does not fetch the repository automatically.

## Updating Knowledge

A Knowledge update is a reviewed OBDentic dependency change:

1. review and merge the canonical change in `obdentic-knowledge`;
2. update the `knowledge/` gitlink to the accepted commit;
3. update `knowledge.lock` to exactly the same commit and supported schema version;
4. run Knowledge DB validation in its own repository;
5. run OBDentic fmt/tests/clippy/build;
6. review the OBDentic PR as a Knowledge dependency update.

CI verifies that the initialized submodule HEAD matches `knowledge.lock`.

## Runtime loader

`knowledge_db::KnowledgeCatalog` is transport-free. It:

- reads only local files;
- accepts schema version 1 only;
- enumerates canonical YAML files deterministically;
- rejects symlinked canonical entries;
- uses strict `serde(deny_unknown_fields)` input structs;
- rejects duplicate definition/semantic/set identities;
- validates definition-set members;
- turns accepted UDS DID definitions into the existing closed `ReadOperation::UdsReadDataByIdentifier` core primitive;
- reuses the existing core UDS response validation (`0x62`, DID echo, NRC preservation);
- exposes repository revision, schema version and definition provenance to later resolver/capture layers.

The schema already reserves an `obd2.mode01.pid` descriptor for staged migration, but the first core loader deliberately rejects it until its full response-length/decoder contract is represented safely. Schema support does not automatically grant executable core support.

## VIN boundary

`F190` / VIN is a vehicle-identity concern and is deliberately separate from the standard ECU-identification set. Canonical knowledge may describe identity semantics, but private VIN values and ECU serials belong to local observed inventory, not to this public repository by default.

## Capture provenance

Issue #89 will persist the active Knowledge repository revision/schema and resolved definition identities in captures. This #84 loader exposes the information needed for that later step but does not change the capture schema itself.
