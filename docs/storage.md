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
unlimited retention default. Charges include keys and a conservative property
envelope, but are **application accounting**, not total filesystem allocation.
Pinned detail and unresolved work count against their budgets. Admission fails
atomically when it cannot fit; it does not evict safety state.

The host's durable worker calls bounded cleanup with a resumable cursor. Pin
changes conflict with cleanup through database CAS. Cleanup reports logical
deletion; it does not claim that all MVCC, journal or compaction bytes have
already been physically reclaimed. A logical restore disables dispatch across
reopen until current external authority and effect evidence can be established.
An old backup cannot prove that a provider effect has not already occurred.

The pinned provider is not qualified for a hard total-owned-file ceiling. Its
sampled endpoint limit omits growth that matters to this guarantee. Native Git
dependency builds also include an upstream development helper with C code;
this source graph is not qualified as a pure Rust dependency graph. Neither
limitation is represented as a passing check. No SD-card support, power-loss
certification or production deployment support matrix is claimed.

Run `sh scripts/check-contracts.sh` for generation, builds, storage/contract
tests, doc tests, bare-metal policy checks and browser contract interoperability.
Browser contract checks do not demonstrate an installed application UI.
