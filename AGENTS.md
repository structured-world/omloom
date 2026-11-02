# Omloom engineering and review guidance

Omloom is an infrastructure source of truth and a workspace for reviewing and
recording changes. Optimize for useful daily work with partial knowledge,
correct shared state, and bounded retrieval and mutation costs.

## Code Review Rules

### Establish the behavior before proposing a mechanism

Trace the actual producer, transformations, authorization, consumers and commit
point. A finding should name a concrete input/state, the violated invariant and
the observable consequence. Distinguish a reproducible defect from an unresolved
product choice or an optional improvement. Prefer no finding over speculation.
Review the proposed remedy as critically as the symptom: identify the existing
owner that should enforce the invariant before adding state or another check.

### Justify persisted data and its full lifecycle

For new fields, rows, indexes, snapshots, counters or partitions, identify the
actual delivered consumer or recovery invariant. Check the authoritative owner,
write trigger, lookup pattern, cardinality, bounds and reclamation. Flag duplicate
authority, unchanged-input writes, read-triggered persistence and historical
payloads without consumers. A transport field is not a storage requirement.
Small fencing, replay and recovery records have real consumers; do not recommend
deleting them merely to reduce bytes. Expired evidence or deadlines do not prove
that an external effect never happened or that reserved capacity is free.

### Enforce database invariants at their owner

Use typed, appropriately scoped identities and database-enforced uniqueness,
CAS and transactions. A read followed by a write or a process-local lock cannot
enforce exclusivity across concurrent writers. Commit coupled accepted state,
reservation, idempotency, required audit and delivery intent atomically. Keep
external I/O outside short transactions, with the durable intent, fencing and
revision checks needed to reconcile an uncertain result. Entry validation and
commit-time revalidation may both be necessary; share their decision logic.

Design the physical schema for real lookups and updates. A protobuf record is
not a storage schema; avoid whole-message blobs when consumers need independent
fields. Do not copy ownership, placement or lifecycle state into competing
authoritative records. Derived caches and projections need a concrete latency
benefit, bounded size and consistent invalidation/rebuild behavior. Reuse the
existing owner rather than surrounding duplicate state with more guards.

### Protect current reads and minimize write amplification

Current-state retrieval must not scan or replay unbounded history. Fetch the
selected scope and fields, paginate with stable ordering, and descend only as
far as the actual view or operation requires. Counts and pages must agree under
their promised read revision. Check visibility before aggregation and export.
Inspect real query plans and representative latency when proposing indexes or
query rewrites; an index name alone does not prove selective execution.

Reads must not persist access counters, last-read times, cache touches or new
snapshots without a delivered consumer. Identical input must not copy payloads
or create duplicate domain history. Freshness, source checkpoints, audit and
authority transitions may still require a minimal update even when the displayed
value is unchanged. Batch or coalesce only when independent source evidence,
ordering, meaningful transitions and required durability remain intact.

### Bound retention and verify recovery and reclamation

For growing data, check the required lifetime or completion condition, count/byte
bounds, cleanup owner and actual reclamation path. Current authoritative state
lives until domain removal; it does not acquire an arbitrary TTL. Evidence
freshness, user-visible history, replay protection and physical reclamation have
different lifetimes. Retain detailed history only for a named consumer, with
explicit time/version limits; do not introduce universal snapshots or an event
log merely because the database supports them.

Cleanup must be bounded, resumable and safe across replicas and interrupted runs.
Logical deletion does not prove that blobs, old MVCC versions or disk allocation
were reclaimed. Include indexes, journals and maintenance headroom in storage
claims. Never expire unresolved effects, active holds or required restore/replay
checkpoints to make quota available. Bound admission and expose unresolved work;
backups must not resurrect expired authority or completed effects.

Credential providers own reusable secrets and monitoring providers own telemetry
history. Do not mirror their stores or retain tokens, transient credentials or
whole provider responses in inventory, audit, delivery payloads, logs or backups.
Verify affected consumers with concurrent admission, unchanged-input write cost,
bounded reads and interrupted cleanup/recovery where those invariants change.

### Separate navigation, authority and aggregation

A focused resource, selected view and authorization grant serve different
purposes. Do not turn navigation state into a persisted dimension by default.
For aggregate disclosure, establish the intended contributing population and
the reader's authority over it. Adding focus-specific buckets is not a substitute
for authorization. Enforce resource and field access at the store boundary before
returning protected values or counts; UI filtering cannot supply that guarantee.

### Preserve partial knowledge and reduce user decisions

Unknown, explicitly zero, absent, stale, restricted and conflicting facts are
different states. Request missing facts only when the particular operation needs
them. Drafting and recording actual work must not require certification of an
entire infrastructure. Conversely, unknown required facts cannot certify fit,
free capacity or working service. Do not require redundant user input when the
authoritative context already determines it, invent an identity from a label,
or multiply modes and controls without a concrete user task.

### Keep domain geometry and mechanisms general

Rack U numbering is an optional declared mounting grid, not a universal enclosure
coordinate system. Nested bays and equipment use their enclosing coordinate
frame and actual placement constraints; names and pictures do not establish
orientation, dimensions, mounting compatibility or occupancy. Reuse the shared
plan, requirement, reservation and recovery mechanisms instead of introducing a
second workflow or copying physical state into another authority.

### Review work and tests at the required granularity

Flag whole-graph reads, N+1 queries, unnecessary decoding, allocations and writes
against the actual requested projection. An index or materialized copy needs a
demonstrated access-pattern benefit and a consistent update lifecycle.
For a correctness defect, identify a regression scenario that fails on the
unfixed path and preserves the surrounding valid behavior. Test the owning
boundary and real consumers. Do not recommend weaker assertions, silent fallbacks,
larger timeouts or unrelated full-suite reruns as a substitute for diagnosis.
Use stable Rust, nextest for Rust tests, and separate doctests.
