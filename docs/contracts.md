# Infrastructure contracts

The protobuf schema in `proto/omloom/v1/contracts.proto` generates Rust wire
types at build time and TypeScript types with `npm run generate`. It describes
inventory and operations; there is no runnable server, installer or user UI yet.

Rust admission is `omloom_contracts::decode(frame, limits, &budget)`. It returns
an immutable validated record or a stable error class. Wire types are untrusted
transport values. Construction of a generated struct, successful decoding, a
UUID or a plan digest grants no authority. Consumers must obtain authenticated
identity and current permissions through their identity provider and verify
object ownership before reading or applying records.

Asset identity and incarnation are distinct lowercase hyphenated UUIDv7 values
([RFC 9562 sections 4 and 5.7](https://www.rfc-editor.org/rfc/rfc9562.html)).
Only Omloom-owned IDs are defined here. Provider identity includes its configured
connection, object kind, object ID and qualified incarnation evidence. An IP,
display name or reused provider number cannot transfer permissions or credentials.
Candidate records remain outside managed inventory until explicitly adopted.
Missing incarnation evidence is retained on a review candidate; an accepted
provider mapping requires qualified lifetime evidence.

Accepted intent includes per-field authority. Imported proposals and independent
observations retain their own provenance, revision/scan window and validity.
Each current layer contains at most one claim per `(source_id, field name)`.
Fields with identical provenance share a group; a source may have multiple
disjoint groups with different windows. If a partial refresh updates B at time
200 but omits A observed at time 100, A keeps its original window. Replacing B
replaces its current claim; appending B's old sample to the layer is rejected.
Empty current groups and split groups with identical provenance are rejected.
Missing fields are absent claims; an explicit `clear` is a source's claim and
does not erase another authority's value. Empty text and false are real values.
Provider revisions are opaque, and source timestamps cannot order other sources.
`SourceObject` and `Evidence` have one shared validation contract in standalone
records and scan rows. A `SourceIdentity` supplies a standalone object's connection;
an `Observation` supplies its optional scan identity. `ScanCandidate` inherits
both connection and scan identity from `ScanBatch`, so rows cannot override or
repeat that context. Identity-only discovery may have empty fields; current
asset evidence groups must have at least one claim.

Scan checkpoints accompany batches for later atomic persistence. Only a qualified
consistent completed snapshot can support absence proposals; partial or denied
scans cannot establish deletion. This library neither stores nor deletes assets.

Provider connections contain canonical HTTPS origins and opaque trust/credential
binding IDs. They contain no credential values or arbitrary file paths. Network
admission must additionally enforce configured destination, resolved-address,
redirect and TLS identity restrictions before I/O. An infrastructure connection
instead describes endpoint incarnations, direction, service, optional VRF/VLAN
and typed requirements. Neither connection is evidence of observed health.

Requirement graphs are flat DAGs with nonempty all/any groups, explicit
completeness and typed resource facts. The library validates structure only.
All nodes must be reachable from the single root. Unused acyclic nodes are
`Reference` errors; cycles anywhere in the submitted graph are `Cycle` errors.
Shared descendants are allowed. Admission rejects unused nodes rather than
silently pruning an expression that a caller may have confirmed.
Evaluation and its satisfied/failed/unknown evidence rules belong to the consumer.
It does not implement authorization, monitoring or notification suppression.

Operation records preserve inventory revision, confirmed plan digest, durable
revision, original provider task and effect knowledge. Unknown effects cannot
be reported as succeeded, failed or cancelled. Confirmed effects may still
need attention. Persistence, legal transitions, idempotency scoped to authenticated
caller/installation, leases, confirmation and reconciliation must be implemented
by the execution/store owner before dispatch; no mutation is sent by this crate.

## Admission limits

The versioned admission profile allows at most 1 MiB encoded bytes, 4096 wire
fields (including packed elements), 16 nested messages and a conservative
16 MiB decode allocation envelope. Callers can tighten these limits. Preflight
borrows the complete bounded frame and validates tags, lengths, numeric ranges,
UTF-8, depth and work before invoking the generated decoder. Unknown fields,
duplicate singular fields/oneof members, groups and unsupported wire types are
rejected explicitly. This is a strict application admission profile over
[protobuf encoding](https://protobuf.dev/programming-guides/encoding/), not a
claim that protobuf itself forbids duplicate fields or extensions. Raw protobuf
bytes are not a canonical plan hashing format.

MemoryBudget can be shared across concurrent admissions; reservations precede
decode and stay with each result/encoded copy until drop. The envelope charges
twice the input size plus 4096 bytes and 2048 per wire field, covering string
copies, generated messages, Vec growth and bounded validation scratch. Build-time
size assertions protect this envelope when the schema changes. Transport callers
must independently cap frame reception and concurrency before buffering input;
raw generated helpers and caller-created/cloned values are outside admission.

Each asset has at most 256 intended fields and 32 provider mappings. Each current
imported/observed layer has at most 32 sources and 256 nonempty provenance groups,
with at most 256 unique fields per source across all its groups. Each evidence
group has at most 256 unique fields;
text values are at most 4096 UTF-8 bytes. Tokens are at most 64 ASCII bytes,
provider identifiers/revisions/cursors at most 256 bytes. Scan pages hold at most
128 candidates. Requirement graphs contain at most 256 nodes, 1024 edges and
64 children per group, validated without recursive graph walks. These are
initial tested contract ceilings, not throughput or deployment capacity claims.

TypeScript `decodeWire` applies the same wire preflight and uses bigint for
64-bit values. It is a browser wire reader, **not server domain admission**.
The Rust corpus checks domain rejection; Node and Chromium check binary
interoperability, wire rejection, field presence, enum labels and lossless
integers. Browser verification is not installation or application acceptance.

## Development

Install Rust 1.95+, protoc 36.2+, Node 24+ and cargo-nextest. Run `npm ci`, then
`npx playwright install chromium`. Run `sh scripts/check-contracts.sh` for the
complete generation/build/test/browser gate. Successful runs clean their own
build outputs; failures preserve evidence. Generated TypeScript is committed
and CI checks it for drift. Dependencies are pinned by Cargo.lock/package-lock.json.
