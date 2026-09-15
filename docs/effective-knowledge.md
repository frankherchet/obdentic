# Effective Vehicle Knowledge

Effective Vehicle Knowledge is a pure composition layer between private observed ECU identity facts and pinned canonical Knowledge.

```text
raw responder evidence
  -> protocol normalization
  -> private observed ECU facts
              +
       pinned canonical Knowledge
              -> applicability resolution
              -> effective semantic catalog
```

The resolver performs no adapter, session, transport, Git or network I/O.

## Normalized observed facts

The resolver accepts `ObservedEcuFacts`, keyed by a local ECU-instance identifier and the closed `FingerprintField` vocabulary from Knowledge schema v2. VIN is deliberately absent from that vocabulary.

Values are already-normalized strings and are compared by exact equality. This module does not guess encodings for opaque F18x/F19x payload bytes. Converting raw ECU-identification evidence into normalized facts belongs upstream in the private observed-inventory/Vehicle Knowledge boundary and must itself be justified by deterministic evidence.

In particular, the resolver never performs ASCII guessing, case folding, trimming heuristics, regex/range matching, fuzzy similarity, ML classification or decoded-value plausibility scoring.

## Resolution states

For every ECU instance and semantic, all canonical candidate definitions remain visible. Each candidate is classified as:

- `Generic`
- `ExactMatch`
- `PartialCandidate`
- `NoMatch`

The semantic result is one of:

- `ResolvedSpecific`
- `ResolvedGeneric`
- `InsufficientIdentity`
- `Ambiguous`
- `NoMatch`

Specific exact matches outrank generic knowledge. More exact predicates mean greater specificity. A tie at greatest specificity remains ambiguous.

If there is no exact specific match but at least one specific candidate is only partial because identity evidence is missing, generic fallback is blocked and the result is `InsufficientIdentity`. Generic knowledge is selected only when every specific candidate is proven not to match.

This is deliberately conservative: missing identity evidence never becomes a reason to guess a decoder.

## Provenance

Every candidate result retains:

- definition ID and version
- applicability match and specificity
- applicability provenance/confidence
- definition provenance/confidence
- hardware-validation state

The effective catalog also records the pinned Knowledge repository, revision and schema version. A later capture-provenance slice can persist these identities without changing the matching result.

## Safety boundary

Applicability does not create executable operations. A selected definition remains one of the already typed, read-only `KnowledgeReadOperation` values loaded by `knowledge_db`.

Any later live consumer still follows:

```text
effective definition
  -> closed typed read operation
  -> SubscriptionPolicy where applicable
  -> SafetyPolicy
  -> single-owner DiagnosticSession
```

No resolver API accepts or generates raw CAN/UDS/ELM commands, arbitrary PIDs/DIDs, session control, SecurityAccess, coding/adaptation, actuator operations, DTC clear or writes.

## Current integration boundary

This slice deliberately stops before #87/#88 integration:

- #87 owns the full private observed-inventory/cache model and the deterministic conversion from preserved ECU evidence to normalized identity facts.
- #88 will consume effective semantic availability from this resolver rather than resolving protocol details inside profiles.

Keeping those integrations separate makes the evidence -> facts -> effective Knowledge direction explicit and testable.
