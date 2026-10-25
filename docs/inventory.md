# Local inventory workspace

The development host runs the actual inventory API, Vue interface and on-disk
CoordiNode store. It requires an explicit synthetic fixture context and binds
only to `127.0.0.1`. It is not a production login, installer or permission
provider. Use test-owned data. No connector credentials or external effects
are supported by this host.

## Run

With Rust, Node.js, protoc and the repository dependencies installed:

```sh
npm ci
npm run generate
npm run build:app
cargo build --locked -p omloom-app
target/debug/omloom-dev --data var/inventory-fixture --ui dist/app --fixture operator --port 9086
```

Select another unused local port when necessary. Omitting `--port` selects a
free port and prints its URL. `--data`, `--ui` and `--fixture` are required.
Closing and restarting the host with the same data path preserves inventory.
Never open the same directory from two independent host processes.

The available fixture profiles are `operator`, `developer`, `engineer` and
`viewer`. They select server-owned scopes. The browser cannot select an actor
or assert grants. The fixture API qualifies application projection; it does not
qualify authentication or deployment for other users.

## Work with inventory

Create resources independently of a hierarchy or integration. Name collisions
do not merge identities. Edit with explicit preserve, set and clear operations;
the accepted revision prevents a stale form from replacing a newer edit.
Observed evidence, import proposals and accepted values remain separate.

Presentation and scope are independent. Switching Hardware, Network or another
view retains the selected root. Details, filters, columns and selection are
addressable through the URL. Unplaced selects the absence of accepted physical
placement, not absence of observations or a claim of available capacity.

Company fields have stable identities and explicit types, applicable resource
kinds, choice values and search inclusion. Renaming preserves field identity.
A type or applicability change requires a validated migration before applying
the definition and current values together. No default backfill or automatic
index per definition is created. Retired definitions with no remaining values
are reclaimed by bounded maintenance; their identifiers cannot be reused.

Saved views are personal initially. Team publication separately previews the
definition disclosed, including roots and predicates. Definition audiences and
editors do not grant access to matching inventory. Personal URL adjustments do
not modify the shared definition. Reset reloads that definition. Unavailable,
retired or retyped predicates fail explicitly instead of broadening the query.

Network allocation distinguishes address space, L2 domain and routing context.
An ordinary address is unique in its address space; link-local identities also
include the selected link. Prefixes retain their mask and may be nested. Next
available allocation is atomic and does not enumerate IPv6 hosts. Shared use
requires an explicit VRRP or anycast mechanism and typed interface participants.
Removing a participant does not release the address. Intended priority is not
evidence of an active gateway. Restricted participant coverage remains explicit.

## CSV and storage bounds

An import names a personal source and explicitly maps selected columns. Each
row requires `external_id`; repeats update that source identity rather than
creating name-based duplicates. Omitted columns preserve fields; `<clear>` is
an explicit clear. Invalid rows retain row-level diagnostics and cannot be
partially accepted. A valid preview expires, and applying it rechecks the current
application revision, actor scope and field types. Raw CSV history is not saved.

Export applies the current object and field projection before counts and output.
Spreadsheet-safe CSV protects formula-leading cells; the typed API can request
lossless machine CSV separately. These are different output contracts.

Fixed operation ceilings are 100 resources per query page, 128 explicit targets
or CSV rows per mutation, 32 requested fields, 8 equality predicates, 256 KiB CSV
and 4 pending previews per actor. A larger operation must be narrowed rather
than silently truncated. Frames have strict structural, depth and allocation
preflight before protobuf decoding. No search/read access counters or result
snapshots are persisted.

The fixture storage profile has finite per-class and total admission budgets.
Previews live for 30 minutes; inventory audit detail lives for 7 days. These are
development data lifetimes, not production retention recommendations. One owned
maintenance task processes bounded pages every 30 seconds, retaining its cursor
on failure. Recovery checkpoints and accepted current state do not receive those
TTLs. Physical engine qualification remains separate from logical row removal;
see [storage guarantees](storage.md).

## Verification

```sh
sh scripts/check-contracts.sh
```

This checks the real API/store and starts the compiled host for Chromium
scenarios with temporary disk directories. It does not intercept API requests
or replace the database. The scripts own their temporary data and generated
verification output. Native OS evidence and production identity integration
remain separate qualifications.
