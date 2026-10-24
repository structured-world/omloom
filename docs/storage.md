# Embedded storage

`omloom-store` is a trusted host library backed by embedded CoordiNode. It is
not a server, authentication endpoint or installer. Applications must obtain
current identity and permission evidence before calling it. A database record,
caller string or UUID grants no authority. Identity-provider integration is a
separate component; this library does not implement login or role assignment.

The backend uses immutable Git dependencies and a consumer lockfile. Its
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
the operation, which cannot expire while unresolved. Dispatch first commits
unknown effect knowledge under a target lease and epoch. An expired sender
cannot settle work; taking over unknown work permits reconciliation only.
Terminal settlement acknowledges work atomically and removes recovery arguments
that no longer have a consumer. Minimal retry checkpoints have their own budget
and do not disappear with recent activity detail. An expired request is refused,
never interpreted as a new effect. No provider request is sent by this library.

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

The pinned provider is not qualified for a hard total-owned-file ceiling. Its
sampled endpoint limit omits growth that matters to this guarantee. Native Git
dependency builds also include an upstream development helper with C code;
this source graph is not qualified as a pure Rust dependency graph. Neither
limitation is represented as a passing check. No SD-card support, power-loss
certification or production deployment support matrix is claimed.

Run `sh scripts/check-contracts.sh` for generation, builds, storage/contract
tests, doc tests, bare-metal policy checks and browser contract interoperability.
Browser contract checks do not demonstrate an installed application UI.
