# 0008. Local diagnostic index

Date: 2026-11-19  
Status: proposed

## Context

[#364](https://github.com/frankherchet/obdentic/issues/364) asks whether an offline lexical candidate can satisfy [#357](https://github.com/frankherchet/obdentic/issues/357) on Android, without choosing a dependency in advance. The timebox is one focused design/refinement session. The approved corpus is rights-cleared definitions from pinned Knowledge only, German/English where available; external standards and manufacturer material are excluded. Source rights, languages and revisions must be checked independently of synthetic performance data. The fixed upper bounds are 16 MiB source text, 32 MiB resulting index, +8 MiB like-for-like Android release APK, 10 seconds for an offline verified update, and 10 hits / 4 KiB UTF-8 quotation text per response, including targeted follow-up. Invalid replacement must leave the prior index intact, but a different active pin must never be served as current. These are approved values, not measurements.

## Options considered

1. **Candidate to investigate, not select:** a local lexical inverted index with fixed, versioned tokenization and explicit deterministic ordering (including stable ID/revision tie-breaks). Filter against the exact Knowledge pin and explicit ECU applicability; unknown or incompatible applicability is not an available vehicle function. Store source ID, revision, language, citation and provenance and resolve targeted follow-up by exact ID and revision. Cap total quoted UTF-8 bytes and hit count. Build a separate replacement, verify its pin and limits, then atomically publish it; retain the old index on failure and reject it as current on pin mismatch. Search text is data, never authority for operations. SQLite FTS4 is one possible experiment, not a requirement; the same contract can be evaluated with another lexical implementation.
2. Platform SQLite FTS5: potentially useful, but SQLite version alone does not establish that the optional module is enabled on a particular device. A Room annotation does not establish platform availability. Do not assume it exists.
3. Bundle a SQLite/FTS implementation or use another library: could control features, but package size, Android integration, all transitive rights and workload remain unproved. Do not add a dependency on that basis.
4. No index / bounded scan: a useful comparator and potentially simpler for a small corpus, but no evidence establishes its update, query or package behavior at the approved upper bound.

## Measurements

These are source and repository observations, not workload measurements. No benchmark, build, test, device, model, vehicle or private data was used.

- `knowledge.lock` identifies `frankherchet/obdentic-knowledge`, revision `840fb995fc84a332c4b3870220409bc7d0404b98`, schema 2. [`docs/knowledge-db.md`](../knowledge-db.md) describes the gitlink as the source pin, a reviewed lockstep update, deterministic validated loading and Android embedded pinned documents. [`src/knowledge_db.rs`](../../src/knowledge_db.rs) validates the pin and supported schema. Existing effective Knowledge, not a retrieved passage, decides applicability and execution.
- The `knowledge/` checkout in this worktree is empty. The GitHub API recursive tree for that exact revision (`repos/frankherchet/obdentic-knowledge/git/trees/840fb995fc84a332c4b3870220409bc7d0404b98?recursive=1`) lists definitions, schemas, documentation and tests, but no license file or source register. The repository metadata reports `license: null`. This is not proof that rights do not exist; it is insufficient evidence of indexing or redistribution rights, per-source languages or provenance. The approval specifies a source-register policy, not evidence that a complete rights-cleared register exists. [`REQ-048.7` and `.10`](../requirements.yaml) require exclusion without clearance and a register for every included source. A main-repository license does not automatically establish rights for a separate repository or underlying material.
- [`android/app/build.gradle`](../../android/app/build.gradle) specifies minSdk 26, target/compile SDK 36 and `arm64-v8a`; it does not identify a benchmark device. [Android's SQLite reference](https://developer.android.com/reference/android/database/sqlite/package-summary) lists SQLite 3.18 for API 26 and notes that version can vary across releases and manufacturers, including within an API level. This is platform documentation, not a measurement of the supported device.
- [SQLite FTS5 documentation](https://www.sqlite.org/fts5.html#compiling_and_using_fts5) says FTS5 is in the amalgamation since 3.9.0 but build settings control inclusion; query results without `ORDER BY` have arbitrary order. [Android Room documentation](https://developer.android.com/training/data-storage/room/defining-data#support-full-text-search) describes FTS3/4/5 and explicitly distinguishes platform and bundled driver support. [AOSP's current SQLite build file](https://android.googlesource.com/platform/external/sqlite/+/master/dist/Android.bp) explicitly enables FTS3 and FTS4 and omits loadable extensions in its common flags; this current source is not evidence of the actual configuration of every API 26+ device or of an installed build's FTS5 support. Probe the actual reference configuration rather than infer features from version or load extensions.
- [SQLite's own copyright statement](https://www.sqlite.org/copyright.html) says its deliverable code and documentation are public domain and may be redistributed; this does not establish rights for a chosen wrapper, bundled binary, other dependencies or Knowledge sources.

## Criteria verdict

The approved #364 criteria are unchanged. “Inconclusive” below is not a pass or an inferred failure.

- **Confirm — inconclusive:** no representative synthetic 16 MiB Android workload, deterministic applicability/revision and targeted-follow-up observations, measured budgets, or source-register rights evidence exists in this session. The platform and pin observations above do not satisfy this conjunctive criterion.
- **Refute — inconclusive:** no observation shows an applicability/revision failure, an exceeded budget, or a selected dependency that cannot be redistributed/packaged on the supported Android build. Missing rights evidence or an empty checkout is not proof of a failure.
- **Inconclusive condition — met:** representative Android workload proof and rights evidence are missing; therefore no candidate is selected by assumption.

### Paper dry run

No conflicting approved criteria were identified; no inputs or limits have been amended. The following are designs for later evidence, not executions:

| Input and precondition | Observation confirming / refuting / inconclusive |
| --- | --- |
| A reproducibly generated, representative synthetic DE/EN corpus exactly 16 MiB with collisions, two revisions and conflicting ECU applicability; fixed generator seed, expected answers and pin. Search twice offline; request each cited ID/revision. | Identical correctly ordered applicable hits and exact follow-up **confirm that portion**; wrong applicability, revision leakage or non-determinism **refutes**; absent corpus, oracle, runs or reference configuration **inconclusive**. Ten hits and 4 KiB total quote limits must be checked at and over the boundary. |
| Independently inspect the source register at the exact Knowledge revision; each included definition has a source, language, revision and documented permission for both indexing and redistribution. | Complete documented rights **confirm that portion**; missing evidence **inconclusive**, not an affirmative rights denial; established prohibition for a proposed dependency's redistribution/packaging **refutes**. Exclude uncleared source material irrespective of benchmark outcome. |
| On a named supported Android reference configuration, offline verified update from the synthetic corpus; record source bytes, resulting index bytes, elapsed update through pin verification and publication, plus signed release APK sizes built identically with/without candidate. | Each value at or below 16 MiB, 32 MiB, 10 s and +8 MiB respectively **confirms that portion**; any approved budget exceeded **refutes**; no representative run or missing comparable APKs **inconclusive**. Record toolchain, device/image, ABI, OS, storage, workload and exact raw inputs/results. |
| Existing verified old index; try invalid replacement and a changed active pin. | Failed replacement keeps old bytes intact and reports mismatch rather than serving stale results; incorrect revision isolation **refutes**. No run or inability to observe both old bytes and returned status **inconclusive**. |

A possible reference is an API 26 arm64-v8a Android emulator, matching the minimum supported API and ABI; emulator image/build, host resources, storage profile and whether it is representative must be explicitly named and agreed before measurement. It is a proposal, not an approved reference or a substitute for evidence on the supported configuration.

## Uncertainty

No index size, APK delta, update time, retrieval quality, determinism, availability or Android packaging result was measured. The upper-bound corpus needs a documented representative distribution of languages, term lengths, applicability collisions and revision counts; avoid treating synthetic benchmark rights as Knowledge corpus rights. The exact 4 KiB boundary is UTF-8 bytes, not characters. The approved APK comparison must include the actual release configuration and any shipped index/source assets; do not substitute debug APKs or compare unlike ABIs/toolchains. Record resulting index size and any temporary update storage separately; no unapproved temporary-storage limit is implied.

The source-register evidence is currently unknown. Next obtain a register tied to the pinned revision with source-by-source ownership/provenance, indexing and redistribution permission, language and revision; verify the permissions with the rights holder and exclude anything uncleared. Separately identify the precise reference Android image/device and verify candidate FTS capability by actually creating and querying a synthetic virtual table; record SQLite version and capability rather than relying on compile-option introspection alone. Then run the dry-run workload and save exact generator, configurations, commands and raw outcomes. If pin, configuration or corpus distribution changes, repeat the relevant evidence. A failed update and pin mismatch require independent fault injection and inspection.

## Decision

Propose a deterministic bounded lexical index as an **investigation candidate only**. Do not decide FTS4, FTS5, a bundled library, or an implementation architecture: the approved confirmation criterion is inconclusive. No ADR acceptance, source redistribution, product implementation or hardware conclusion follows from this proposal. Human acceptance of an ADR is a separate gate after sufficient evidence; an inconclusive result cannot be converted into approval by assumption.

## Consequences

#357 remains dependent on demonstrated rights and the supported Android workload, not on the mere presence of an SQLite API. An implementation must preserve pin isolation, explicit applicability and fail-closed update behavior, and keep externally sent excerpts separately auditable under REQ-045. Further evidence may confirm, refute or leave the candidate inconclusive; it must not silently change the approved budgets or widen the corpus.
