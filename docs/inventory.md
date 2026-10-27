# Local inventory workspace

The development host runs the actual inventory API, Vue interface and on-disk
CoordiNode store. It requires an explicit synthetic fixture context and binds
only to `127.0.0.1`. It is not a production login, installer or permission
provider. Use test-owned data. No connector credentials or external effects
are supported by this host.

HTTP requests must name `127.0.0.1` or `localhost` with the actual listener port
in their Host header or HTTP/2 authority. Foreign, missing or conflicting
authorities are rejected before the API and static files to prevent a remote
website from acquiring fixture access through DNS rebinding.

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

The host allows 128 MiB of query execution memory per database statement.
Use `--query-memory-mib MIB` to select another finite budget. This controls
query working memory, not total process memory or database disk usage. The
default supports the measured 10,000-resource fixture workload; it is not a
production sizing recommendation. Concurrent requests, the embedded engine
and the interface have additional memory costs.

The available fixture profiles are `operator`, `developer`, `devops`, `engineer` and
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

Ordinary browsing shows one ownership level. Hardware shows containers such as
hosts instead of flattening their ports into the same list. Select a host as
scope to browse its direct components; components of a nested device remain
inside that device. Counts and pagination use this server-side selection.
Search and explicit field filters can query the selected scope more broadly.
IPAM selection uses its namespace and address-range semantics independently
of physical ownership. Hidden ownership does not conceal otherwise permitted
resources or grant access to their containers.

List context is limited to authorized owner labels and address hints for the
displayed services/interfaces. It does not load host routing tables, firewall
rules or the address sets of ancestor hosts and sibling services. Network
observations are retrieved in the selected resource inspector. Physical
visualization may request bounded details for visible containers separately;
opening the estate list does not expand every container.

Destination/contact references remain searchable inventory facts and do not
appear as local address hints. Hint classification also respects meaning-field
visibility.

Service rows can show actual permitted transport endpoints without a dedicated
interface. Wildcard binds retain their address family; a configured interface
address and an observed listener are distinct facts. A listener does not prove
firewall permission or network reachability.

<a id="current-network-observations"></a>
Current network observations retain separate source claims and exact dates for
individual addresses, listeners, routes, application rules and policy settings.
Partial input preserves omitted values with their earlier dates and boot basis.
A complete section replaces only that source's section. Values and freshness
metadata are stored separately; identical delivery and set reordering perform
no writes, and a freshness refresh does not rewrite values or append history.
Temporary address literals are rejected before persistence. The fixed network
evidence bounds are eight sources and 2,048 total facts/256 KiB of value payload
per installation; interface addresses have eight sources and 64 total claims.
Storage admission additionally charges keys, metadata and indexes under their
own budgets. These are development ceilings, not supported production sizing.
Resource reads paginate network observations within the complete 512 KiB reply
budget. Continuations are bound to the target, working context, authority and
current revision. They return only the next evidence page, including exact
per-fact dates and basis, source coverage and labels needed for its listeners.
The inspector replaces its current evidence page; counts and filters apply to
that page. Reading or continuing does not write history or freshness metadata.

The on-disk migration preserves existing observations. Earlier partial
interface collections did not retain individual dates; migrated values show
an unknown observation time until explicitly reobserved. Their collection
checkpoint is preserved separately. Archiving a resource reclaims its current
network facts, retaining the existing identity/replay fences.

Resource kinds and Columns open checkbox selections with Apply and Cancel;
changing a draft choice does not change the query. At least one data column is
required. On phones the resource list uses cards with page selection and sorting.
Opening a resource shows a separate detail screen; Back to inventory preserves
the query, page, selection and list position. Editing and selection forms use
the full phone screen. More exposes secondary phone actions. Wide screens place
the selected detail beside the list.

Creation first follows the current resource or selected roots, then the view.
A VM offers interfaces and known address facts, rather than another VM or
upstream network topology. New children and their required attachment commit
together. Several possible parents require an explicit choice. Without a
selected resource, Virtualization starts with a VM, Services with a service and
Hardware or Overview with a device. A context never supplies grants or copies
its parent's property values.

Network offers recording a known IP independently of allocation. A bare IPv4
or IPv6 address needs no name, mask, namespace or VRF. Optional configured
versus destination meaning, assignment mask and host-local IPv6 zone remain
unknown unless supplied. Equal addresses on unrelated resources stay separate.
Allocate IP / prefix is a separate action in an address space or prefix;
Create address space returns without losing the allocation draft.
An explicit prefix container supplies its declared namespace and enforces its
range. It does not supply routing or an interface configuration mask.

Connections in broader scope explicitly selects both endpoints before offering
their supported relationship meanings. Editing or removing a connection checks
the previous endpoints as well. Returning preserves the local resource/query;
local details do not expose a peer's underlay. Address reconciliation similarly
reviews a canonical mapping. A new allocation, optional explicit intended
interface assignment and mapping commit atomically. A conflict keeps the
original known fact; it does not merge machines or enable shared use.

On phones, the detail screen provides a perspective selector, local resource
creation and a bounded child list. Back follows the resource trail, then returns
to the original inventory query and selection.
Changing a representation inside details does not replace the original return
destination. The URL preserves its enclosing query across reload; selection and
scroll restoration apply within the current browser session. Relationships use
separate pages of at most 25 permitted edges. Their continuation is bound to
the resource, working context, authority and data revision; it does not reload
the resource's routing/firewall evidence. Large containers remain usable without
enumerating all children to prepare actions or validate the selected target.
These create accepted inventory records, not real provider resources. External
provisioning and connection adapters are not implemented by this fixture host.

Company fields have stable identities and explicit types, applicable resource
kinds, choice values and search inclusion. Renaming preserves field identity.
Applicable kinds form an unordered set. Reordering them does not require a
migration, write data or invalidate saved predicates.
Definition creation, edits and migrations enforce an aggregate protobuf reply
budget of 512 KiB, including retired definitions. Capacity refusal leaves the
schema and values unchanged, so accepted metadata remains readable at startup.
A type or applicability change requires a validated migration before applying
the definition and current values together. No default backfill or automatic
index per definition is created. Retired definitions with no remaining values
are reclaimed by bounded maintenance; their identifiers cannot be reused.
Migration previews show the affected and invalid value counts, with at most
32 authorized examples requiring repair. Narrowing applicability preserves
existing values unless the operator explicitly reviews their removal. Applying
a migration rechecks the application revision and all affected data permissions;
it changes accepted local values together or leaves them unchanged. Provider
observations and import proposals keep their source interpretation.

Saved views are personal initially. Team publication separately previews the
definition disclosed, including roots and predicates. Definition audiences and
editors do not grant access to matching inventory. Personal URL adjustments do
not modify the shared definition. Reset reloads that definition. Unavailable,
retired or retyped predicates fail explicitly instead of broadening the query.
Retired definitions keep their current values. Definition cleanup returns a
continuation even when a page contains only retained definitions; the host
resumes that cursor and safely restarts the pass after a process restart.
Skipping retained definitions creates no durable cursor or idle writes.

Network allocation distinguishes address space, L2 domain and routing context.
An ordinary address is unique in its address space; link-local identities also
include the selected link. Prefixes retain their mask and may be nested. Next
available allocation is atomic and does not enumerate IPv6 hosts. Shared use
requires an explicit VRRP or anycast mechanism and typed interface participants.
Each shared group has its own stable identity, revision and participant set.
Several canonical addresses can select the same group. Editing membership
through one address checks the group's revision and affects its other addresses;
an address edit cannot silently overwrite a concurrent group change. VRRP
protocol identity is scoped by L2 domain, address family and VRID, independently
of the group's stable identity. VRID changes update the group once; address
cards and filters read that current value rather than stored copies.
Removing a participant does not release the address. Intended priority is not
evidence of an active gateway. Restricted participant coverage remains explicit.
Resources with retained IPAM state cannot be archived through the generic
asset writer. Explicit IPAM removal owns reference checks and allocation release.

## CSV and storage bounds

An import names a personal source and explicitly maps selected columns. Each
row requires `external_id`; repeats update that source identity rather than
creating name-based duplicates. Omitted columns preserve fields; `<clear>` is
an explicit clear. Invalid rows identify the failing row and mapped column without
echoing its value or persisting a rejected preview; invalid input cannot be
partially accepted. A valid preview expires, and applying it rechecks the current
application revision, actor scope and field types. Raw CSV history is not saved.
Preparing a replacement atomically reclaims that importer's earlier review-only
drafts and their rows. Failed preparation preserves the prior draft; another
importer's previews remain under their own expiry and revision checks.
New IPs, prefixes, VLANs and shared groups use the network allocation form,
which checks their typed scopes and uniqueness. The generic CSV preview rejects
these creation modes instead of promising a change that cannot be accepted.

Export applies the current object and field projection before counts and output.
Spreadsheet-safe CSV protects formula-leading cells; the typed API can request
lossless machine CSV separately. These are different output contracts.

Fixed operation ceilings are 100 resources per query page, 128 explicit targets
or CSV rows per mutation, 32 requested fields, 8 equality predicates, 256 KiB CSV
and 4 pending previews per actor. A larger operation must be narrowed rather
than silently truncated. Query pages also fit a 512 KiB encoded reply ceiling:
fewer rows are returned with a continuation, preserving requested values.
Frames have strict structural, depth and allocation
preflight before protobuf decoding. No search/read access counters or result
snapshots are persisted.

The fixture storage profile has finite per-class and total admission budgets.
If a format upgrade cannot fit its derived projections, the trusted host can
explicitly call `Store::open_with_capacity_increase` with the exact previous
policy and larger admission ceilings. Ordinary open still rejects policy changes.
This operation cannot reduce ceilings or change retention, preserves restore
fences, and commits the new policy before resumable migration work. After an
interruption, normal open with that new policy resumes the upgrade.
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
