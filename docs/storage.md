# Embedded storage

`omloom-store` is a trusted host library backed by embedded CoordiNode. It is
not a server, authentication endpoint or installer. Applications must obtain
current identity and permission evidence before calling it. A database record,
caller string or UUID grants no authority. Identity-provider integration is a
separate component; this library does not implement login or role assignment.
The [local development host](inventory.md) instead supplies explicitly selected
synthetic contexts for test-owned data. These qualify application projection
without claiming identity-provider enforcement.

The backend pins CoordiNode Git revision
`d7dae1f28097f6cb58bcdf58042949d68405dba9` and a consumer lockfile. This source
revision includes nullable Boolean, correlated lookup and edge-orientation
corrections required by inventory queries. It is not a published embedded release. Its
filesystem capability requires the `std` feature. Policy types also compile
without default features on a bare-metal target. A host can supply a supported
replicated Database instead of opening a local directory. Never concurrently
open one directory as independent databases.
An injected database retains the host's retention and resource configuration;
the host must supply finite settings suitable for all of its namespaces.

## Layout and write contract

The schema stores directly indexed current rows. Asset identity and accepted
revision are separate from intended fields, provider mappings, current source
claims and freshness. Source fields use scoped keys; omitted partial fields
remain unchanged. Reattesting an unchanged value updates its freshness row
without rewriting the value or incrementing the accepted intent revision.
Explicit clears remain claims. Different sources never overwrite each other.
Repeated identical input and retrieval do not create application history.

Resource responsibility adds at most four current assignment rows per asset.
Only explicit assignments have an issuer/subject lookup digest, used by the
owner-filter consumer; the full reference is checked before a match. Inherited
assignments retain the accepted relation identity and revision, not a duplicate
parent or effective-owner copy. Reads resolve only the required facets through
transaction-local indexed batches. Editing a parent does not rewrite children.
These current rows live with the resource and follow its explicit retirement;
they are not time-series observations or a party directory.

Selected account metadata, bounded resource bindings and a source-identity
uniqueness mapping also use current inventory rows. They contain no credentials.
Owner-action requests reuse operation intent, retry checkpoints and attributed
operation evidence. They have no work outbox entry and remain protected pending
state until resolved through an appropriate verification workflow. Expiring a
request deadline cannot reclaim unresolved effect evidence or re-enable a retry.
An owner report records a claim, not provider-verified completion. Account edits
and reads do not manufacture credential versions or account-history snapshots.

Interface and service address hints resolve current participant rows, including
shared-group allocations. Indexes on the existing asset and group fields bound
these lookups to the displayed interfaces and their assignments; no duplicate
assignment records or persisted hint snapshots are maintained.

The installation record serializes admission accounting and query revisions
through database CAS, rather than a process-local lock. Uniqueness is enforced
in the database. Pagination expires when its application revision changes.
Accepted edits commit their audit in the same transaction. Current reads do not
reconstruct inventory from audit or operation history.

Each typed relation has a first-class identity row, with native graph edges
from its source asset to that row and from that row to its target asset.
This supports independent parallel relations and indexed adjacency without
depending on unsupported discriminated-edge queries. Identical relation
updates do not add audit entries or rewrite adjacency.

Operation admission commits exact bounded arguments, caller-bound retry digest,
queued state, audit and local durable work atomically. Work records reference
the operation, which cannot expire while unresolved.

Work polling resumes after the last returned operation ID, including unresolved
or deleted entries; an empty page completes the pass and the worker starts the
next pass from the beginning. Polling does not claim work or grant dispatch.
Dispatch first commits unknown effect knowledge under a target lease and epoch.
An expired sender
cannot settle work; taking over unknown work permits reconciliation only.
Terminal settlement acknowledges work atomically and removes recovery arguments
that no longer have a consumer. Minimal retry checkpoints have their own budget
and do not disappear with recent activity detail. An expired request is refused,
never interpreted as a new effect. No provider request is sent by this library.

Admission reserves Detail rows and logical bytes for the terminal summary,
terminal audit and a recovery transition, sized for the supported identifier
bounds. Intermediate audits cannot consume this capacity. Entering reconciliation
uses its reserved transition slot; returning to attention must reserve it again.
Renewed claims in an unchanged reconciliation state update the lease without
duplicating a transition audit. Lease charges include maximum owner-ID width.
Settlement atomically replaces its reservation with actual retained detail.
Usage includes these reservations; optional audit exhaustion cannot prevent
recording an admitted effect's outcome under the current fence.

## Bounds, retention and restore

Opening requires explicit finite row/byte budgets for every data class, a total
logical byte budget, detail age and a cleanup batch bound. There is no
unbounded review cache: source-scoped count/byte budgets and a review lifetime
apply in addition to installation-wide limits. Scan allocation uses a durable
source epoch; retired scans cannot advance the current cursor. Page checkpoints
and candidate replacement commit together. Canonical page digests tolerate
reordered candidates and fields. Exact page replay performs no writes.
Partial, denied and complete scans never remove accepted inventory. Expiry of a
review object does not imply absence of a managed object.

There is no unlimited retention default. Charges include keys, graph adjacency
and a conservative property
envelope, but are **application accounting**, not total filesystem allocation.
Pinned detail and unresolved work count against their budgets. Admission fails
atomically when it cannot fit; it does not evict safety state.

This unreleased storage format uses schema version 9. Supported versions 2
through 8 upgrade in bounded restartable batches, including asset name/sort
projections from accepted intent, normalized IP
ordering and source-scoped current observation facts with separate freshness
and accounting. No inventory history is added. The upgrade keeps restore fences
and admission accounting. Earlier experimental layouts are refused with
`SchemaVersion`, never silently overwritten. See the
[observation contract](inventory.md#current-network-observations) for migration
of previously unavailable per-fact dates.

The host's durable worker calls bounded cleanup with a resumable cursor. Pin
changes conflict with cleanup through database CAS. Cleanup reports logical
deletion; it does not claim that all MVCC, journal or compaction bytes have
already been physically reclaimed. A logical restore permits reads and refuses
mutations across reopen. This library has no method to bypass that recovery
boundary; safe recovery requires the identity and executor integrations.
An old backup cannot prove that a provider effect has not already occurred.
The backup API covers the supplied host database, including other namespaces.
It requires host backup custody and is not an inventory export API. A combined
identity-provider restore still requires that provider's qualification.
Restore takes an explicit, positive maximum encoded backup size, independently
of Omloom's logical application quota and stored policy identity. Oversized
input is refused before opening the destination. The host must enforce the
same input ceiling before buffering a file or network response.

The pinned provider is not independently qualified here for a hard
total-owned-file ceiling. Earlier consumer tests reproduced a physical cap
failure on registry 0.9.0; changing the source pin does not establish that
guarantee. The Git dependency graph includes the workspace development helper
and its `alloca`/C build path, unlike the registry package. No pure-Rust closure,
SD-card support, power-loss
certification or production deployment support matrix is claimed.

Run `sh scripts/check-contracts.sh` for generation, builds, storage/contract
tests, doc tests, bare-metal policy checks and browser contract interoperability.
Browser contract checks do not demonstrate an installed application UI.
