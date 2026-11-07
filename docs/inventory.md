# Local inventory workspace

The development host runs the actual inventory API, Vue interface and on-disk
CoordiNode store. It requires an explicit synthetic fixture context and binds
only to `127.0.0.1`. It is not a production login, installer or permission
provider. Use test-owned data. Persistent connector credentials and external
mutations are not supported; the request-scoped read-only observer is described below.

HTTP requests must name `127.0.0.1` or `localhost` with the actual listener port
in their Host header or HTTP/2 authority. Foreign, missing or conflicting
authorities are rejected before the API and static files to prevent a remote
website from acquiring fixture access through DNS rebinding.

## Run

With Rust, Node.js 24+, Corepack and protoc 36.2+ installed:

```sh
corepack enable
yarn install --immutable
yarn dev
```

`yarn start` is an alias. The command generates clients, checks/builds the UI
into `var/development-ui`, then compiles and starts the Rust host with the
`operator` fixture and persistent database `var/development`. It selects a free
loopback port and prints its URL. The first Rust compilation can take several
minutes. Ctrl+C stops the host; restart after source changes (no live reload).

Pass host options directly, for example:

```sh
yarn dev --port 9086 --fixture viewer --data var/viewer-test
```

Explicit options override the development defaults. Choose a different port if
one is occupied. Closing and restarting with the same data path preserves
inventory. Never open the same directory from two independent host processes.
The command does not open or import another existing local database implicitly.

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

### Partly identified equipment

Create or edit a device and open **Equipment knowledge**. Manufacturer, family,
generation, model and chassis variant are independent axes. Record a known value,
quantity range, possible values or supported exclusions without selecting an exact
catalog model. A range with only a lower bound of twelve drive bays leaves the
total and drive bus unknown. It creates no disks or other installed components.

Possible values can have uncalibrated normalized scores or attributed probabilities.
Probabilities require their hypothesis domain, calibration basis and explicit
unknown mass. An incomplete top-k list cannot sum to certainty. Calibration is
recorded source evidence, not a certification or permission to apply firmware or
declare physical fit. Exclusions require a reason and observed coverage; an unseen
connector is not proof of absence. Contradictory candidates/exclusions remain
visible for review, as do independent current sources.

Refine the same device using a reviewed model reference with issuer, key and exact
basis. This retains device identity, relationships and assignments, and never
copies grants or creates the model's declared components. Ordinary known values
remain ordinary fields. Reads, reordered alternatives and identical edits do not
create history. Field permissions also apply to candidate detail and exports.

The fields are `manufacturer`, `family`, `generation`, `model`, `variant`,
`drive_bays`, `drive_bus`, `height`, `width`, `depth`, `weight` and `power_rating`.
Quantities use fixed-point integer bounds with 0..6 decimal places: count for
bays (scale zero), mm for dimensions, g for weight and W for power. Unknown bounds
are absent, not zero. Each current claim is limited to 8192 encoded bytes, sixteen
candidates and sixteen exclusions. Numeric confidence uses integer millionths.

The API uses the existing field write, read, source-evidence, bulk and import
paths. `Field.equipment` carries a quantity, model reference or alternative set.
It has no separate history stream, catalog synchronization or effect endpoint.
Machine CSV preserves claims as `equipment/claim/v1:` followed by hexadecimal
protobuf `EquipmentClaim` bytes. Ordinary known values remain readable text;
equipment text colliding with reserved markers exports as `equipment/text/v1:`
followed by UTF-8 hex.
Empty device equipment cells are omitted: unknown facts stay absent and existing
values are preserved. Use `<clear>` for an explicit clear; a zero bay count is a
supplied value.
Ordinary text and bay counts can also be entered directly in an import. Preview
and ordinary field validation apply before accepting imported claims.

### Recorded requirements

Open a device's **Check recorded requirements** section to compare a selected
equipment fact with a required value or measurement bound. Choose accepted
configuration or dated observations explicitly. A missing observation is unknown,
not a failed device. Incomplete model alternatives and recognition confidence
cannot certify compatibility. Independent measured bounds can answer their own
specific condition without identifying a model.

For an accepted device fact, **Clarify this fact** opens the ordinary editor at
that field. The question, comparison and threshold stay separate from the value
being recorded. Review the current value and proposed change, then choose
**Save and recheck**. Exact quantities use one value input; **Only a range is
known** preserves partial knowledge. Existing evidence bases and alternatives
are retained unless explicitly changed. Zero, an unknown value and explicit
clear have different meanings. A known shortage offers **Correct recorded fact**
as a secondary action: correcting an inaccurate record is not a remedy for an
actual shortage.

Refreshing the resource invalidates its old answer, including dated observations
whose update does not change the accepted revision. The question remains ready
to check again once resource details are available, independently of list loading.
An explicit service-unavailable or deadline error during clarification keeps the
dated result available for a fresh read; authority, target or revision failures
invalidate it.

The focused `Read.accepted_field` query returns only the selected accepted device
field, edit revision and permitted name. It checks current scope, incarnation
and field visibility before decoding the value. It loads no relations, source
claims or network evidence and cannot accompany a continuation. Ordinary reads
retain their existing detail representation.

Cancel and unchanged input do not write. A revision conflict keeps the draft
until the current fact is read and reviewed again. A lost write response leaves
the outcome unknown: a later read shows what is currently recorded, without
claiming that this attempt saved it or automatically resending it. A confirmed
save and the subsequent evaluation have separate outcomes. **Retry check** only
evaluates; it never repeats the write. Questions and edits are transient while
the workspace is open, with no durable question or receipt ledger.

Account cards can inspect the recorded delegation capability. An owner-only
account explains the need for owner intervention and, when permitted, points to
the existing owner-action request. Checking does not submit that request, reveal
a secret or grant account access.

From an explicit selection of devices, **Check selected devices** asks the same
question once and shows each current fact beside its own result. The reviewed
population stays fixed when the surrounding list refreshes. Up to 63 field
conditions fit in a review, with a shared group node within the evaluator's
64-node limit. **Add another condition** can check the same fact differently;
these conditions share one correction editor without merging their answers or
source evidence. An existing device finding can enter the same review through
**Review these findings**.
An explicitly empty description is a text condition, distinct from missing or
cleared evidence. Measurements and delegation choices still require a value.

**Save and next** records only the selected accepted field, rechecks affected
conditions and opens the next eligible row. Each answer retains its own checked
revision and time; refreshing one row does not relabel older answers. Confirmed corrections survive
another row's conflict. **Defer this row and continue** retains unfinished input
within the open review. Rechecking a conflicting or uncertain write does not
discard its draft, claim success or advance automatically. If a partial check
loses access, the review hides cached results and checks the same selection again.
It removes drafts for unavailable rows and restores allowed rows with their
input. If the full check cannot complete, **Refresh checks** retries it.
**Import CSV and
return** uses the ordinary source mappings and reviewed importer, then rechecks
the original selected conditions. It does not match arbitrary CSV names to
selected devices or extend the selection with newly imported records.

The `evaluate_requirements` workspace query supports a selected root in a bounded
flat graph of field predicates and `all`, `any` or fixed-threshold groups. It
reads exact resource incarnations under the current storage session. Thresholds
count distinct scoped field subjects. A nested logical group has unknown voter
identity and cannot manufacture another vote. This inventory query does not certify
a provider's consensus membership. Limits are 64 nodes, 256 edges and 32 levels, with at most
32 current source claims for a field. Oversized work fails explicitly instead
of returning a partially successful check. Unselected branches do not read facts
or require completed group semantics. Global wire, reference and work bounds still apply.

Results include stable condition keys, current inventory revision, permitted
source revisions/scans, their typed current field values and validity windows.
Selected field metadata and accepted values use bounded indexed batches. Source
values remain attributed separately; stale observations may be displayed but
cannot satisfy a condition. Protected and unavailable subjects
share the same non-disclosing response. Reads do not create plans, requests,
snapshots, history or access timestamps. A satisfied condition is neither an
authorization grant nor an operation admission, live reachability test or
physical-fit certificate. Production authentication and provider qualification
remain separate from this fixture-based workspace.

Large evaluations paginate their evidence within the 512 KiB reply ceiling.
Follow `RequirementEvaluation.next` as `EvaluateRequirements.cursor` with the
same question and working context. Every page rechecks current authority and
content, including source values and freshness decisions. Changed evidence,
permissions or context invalidate continuation. Combine evidence only after
all pages succeed; the browser does this automatically and otherwise offers a
fresh check. No stored snapshot or evaluation history is created. Ordinary
responses that fit the ceiling need no continuation.

### Ownership and responsibility

Open a resource's **Ownership and responsibility** card to inspect or edit its
claimed owner, operator, beneficiary and external account controller. Parties
are selected issuer-qualified references, not an imported directory. Equal
labels in different namespaces never merge identities. A recorded owner is a
claim, not legal verification, authentication or an access grant.

Owner and operator support explicit assignment, inheritance and an explicit
unassigned stop. Beneficiary and account controller are independent explicit
facets. Newly created owned components bind to their accepted component
relation. Inheritance retains that relation and its reviewed revision, rather
than copying a parent's effective value to every child. A parent assignment
change updates derived values below it until an override or stop.

Physical VM hosting alone does not assign a person or team. Selecting a hosting
relation as an administrative responsibility source requires a separate reviewed
assignment. A changed or removed source becomes unavailable; a VM migration
cannot silently select its new host's owner. VPN peers, gateways, shared
storage, VLAN/VRF membership and canonical address allocations are not
responsibility parents. A local interface address fact is distinct from the
network resource it references.

The card separates effective values from source identities and contacts. A
permitted child value can remain visible while its source is hidden. Editing
requires current field rights, resource revision and application basis. The
preview writes nothing. Assignment changes neither transfer credentials nor
create or revoke grants.

The typed filter accepts a facet plus exact party namespace and identity.
Filtering occurs before counts and pagination. Scoped or otherwise selective
queries resolve their actual candidates first. Global discovery starts from
the explicit-party index and follows qualified downward candidates. Unknown
values do not match a known-party filter; an attempted unresolved source is an
incomplete result, not a fabricated owner or a silently truncated count.
Requests have fixed work ceilings of 64 ancestor levels, 4096 resources and
32768 relation observations. Exceeding a ceiling returns a resource-limit error.
The source picker reads up to 128 candidate parent bindings. A resource's
children are not candidates and do not consume that selection limit.

The assignment replacement form requires permission to read both the declaration
and its contact. An omitted unreadable saved contact cannot be implicitly cleared,
even when the caller can write it. Explicit write-only values remain supported.

**Export responsibility** downloads permitted local declarations as typed JSON.
**Import responsibility** previews and atomically accepts up to 100 exact
existing resources under the same validation and revision checks as the form.
Hidden inheritance bindings are omitted from the download. This is a selected
declaration transfer, not a backup or a grant export. Replies are bounded before
acceptance; an invalid or oversized transfer cannot partially commit.

### Selected domains and external accounts

Domains and provider accounts are separate resource kinds in **Software &
services**. An account records only its selected source identity,
administration/programmatic kind, known delegation capability, permitted login
and sender constraints, an optional opaque credential-binding UUID and a known
recovery route. Do not enter passwords, tokens or recovery answers. Provider
identity remains stable across rename and credential rotation.
The metadata form replaces the complete record and requires read and write
permission for every metadata field. Undisclosed optional values are not known
empty values. Binding edits remain independently available under their own rights.

An account can bind up to 32 selected managed resources, services, endpoints and
consumer workloads. Bindings do not copy resources or establish ownership.
Adding or removing a binding reviews the selected account and resource together
in an explicit relationship context. The server checks both endpoints against
that context and their current field permissions.
Each endpoint and its label require independent disclosure rights. Two products
can use separate SMTP account references while mail administration and other
consumers remain hidden. Removing a binding does not revoke external mail access
or delete the account. Referenced resources and accounts with pending owner
requests cannot be removed or retyped into dangling references.

Metadata visibility, sharing its view, workload credential use, human reveal,
rotation, delegation, requesting, approving and performing a change are separate
capabilities. **Share account view** uses the existing reviewed view-publication
flow and separately checks account sharing authority. Recipients still need
their own inventory rights. Publishing a definition releases no secret and
grants no provider access. The capability display describes trusted host
decisions, not qualification of a provider or vault integration.

Replacing account metadata cannot implicitly clear an unreadable saved field.
The complete metadata form requires disclosure of every metadata field; selected
binding changes retain their independent permissions.

Domain resources keep a free display name and an optional `dns_name`. DNS syntax
is checked with IDNA and structured-public-domains; private namespaces and public
suffix nodes remain valid names. Validation does not prove registration or resolve
the name. Create, edit, bulk and CSV writes share the server validator; DNS names
can also be selected as columns, filtered and searched.

Responsibility contacts and account recovery routes declare an optional syntax:
email, international E.164 phone, HTTP(S) URL or DNS name. Account sender constraints
may declare an email or domain. Email uses structured-email-address's SMTP mailbox
parser, without display names or provider-specific normalization. Phone input uses
libphonenumber-js with full metadata, and the server independently checks E.164
syntax and number validity. URL validation rejects embedded credentials and never
fetches the address. Syntax does not establish delivery, ownership or account control.
Provider namespaces, provider identities, login labels and resource names stay opaque.

An unspecified type preserves an opaque route or constraint, including existing
values. Selecting a type is explicit; the server never guesses or silently converts
old data. The discriminator shares its value's read/write permissions and is hidden
with it. Format 13 adds optional discriminator columns on open and restore, without
creating route history or rewriting existing contact payloads. Unchanged submissions
do not create new records or advance the inventory revision.

For a personal account without delegation, the card says **Account owner action
required**. A company approver cannot acquire that account. A request retains
the exact managed target, non-secret proposed change, target/account revisions,
caller and deadline through the existing durable operation model. It starts
blocked with no external effect and no executor work item. Approval remains
separate from performance of the action. A stale target or changed controller
basis requires a new review.

An unavailable owner leaves the request unresolved. An attributable owner report
retains explicit effect knowledge but remains **Needs independent verification**,
never provider-verified success. The ordinary executor cannot claim these owner
requests. The development host does not dispatch provider effects, send mail,
fetch credentials, impersonate the controller or qualify production identity.

Create resources independently of a hierarchy or integration. Name collisions
do not merge identities. Edit with explicit preserve, set and clear operations;
the accepted revision prevents a stale form from replacing a newer edit.
Observed evidence, import proposals and accepted values remain separate.

In Virtualization, create a **Hypervisor (OS installation)** independently of
the physical server. Search for a permitted existing server or leave the single
placement field **Unknown**. A hypervisor is an OS installation with a virtualization
role; the same hardware may have other installations. A VM can select its
hypervisor immediately, or explicitly leave placement unknown. Creating from
the parent resource pins that parent. Placement search stays within the current
scope, uses pages, and checks creation and relationship permissions before save.
The resource and its selected placement commit together.
Windows requires installed and enabled Hyper-V; Linux likewise requires its
virtualization components installed and enabled. The OS name alone assigns no
role. Choose the current virtualization role explicitly; unknown and known not
to be a hypervisor remain distinct. The Proxmox preset selects its default
hypervisor role and allows correction. Accepted role is not evidence of live
health or successful provisioning.

An existing device with a retired virtualization role offers an explicit clear
action to readers with write permission for that field. New roles can only be
assigned to OS installations.

Guests remain discoverable when their installation's virtualization role is
unknown, disabled or unreadable. Their accepted placement is preserved;
listing a permitted guest does not disclose a hidden role.

Containment and execution placement are checked together for cycles before a
relationship commits. A VM cannot run on its own OS installation. Removing an
existing placement remains available when the host role is disabled or unknown.

Device, VM and OS installation creation accept up to 16 initial IP facts, with
optional prefix lengths and local VLAN tags. Unknown interface ownership stays
unknown. A local VLAN tag is not a canonical VLAN or routing-domain assignment.

The development host includes a read-only Proxmox VE observer. Enter an HTTPS
endpoint and optional one-time login credentials or API token, then choose
**Observe**. Review node name, CPU and memory hints, and interface addresses
before selecting values for the unsaved form. Credentials clear after the read
and never enter inventory storage. Provider certificates must validate; redirects
are disabled. Missing permissions and unavailable reads remain visible as limits.
This preview does not identify a physical chassis, adopt VMs or execute actions.
Only saving the resource changes inventory, including its selected placement and
reviewed interfaces and addresses in one transaction.

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
Paste an IP/CIDR, optionally followed by `brd IPv4`, to review the host address
without losing its host bits. Choose a new prefix with unknown namespace, an
explicit existing prefix, or the address alone. A new prefix and its membership
are committed atomically with the address. A prefix name is optional. The review
shows a calculated subnet broadcast separately from an explicit endpoint setting;
IPv6 and IPv4 /31 or /32 do not acquire a directed broadcast setting.

An unresolved prefix is current knowledge, not an allocation or a VRF. It has no
allocation uniqueness key. Equal CIDRs remain separate until explicitly reviewed.
Its focused list includes declared members, rather than guessing membership from
matching address text. Supply the namespace later to refine the same identity;
ordinary scoped uniqueness and any existing canonical bindings are then checked.
Compare two prefixes to review duplicate records and retain one identity. Different
CIDRs, incompatible namespaces, protected references and hidden evidence cannot be
silently reconciled. This uses the same reviewed correction and bounded reversal
as other inventory identities, without transferring grants or credentials.

Overview/network page context supplies the permitted Prefix CIDR for titles,
independently of the 32 requested field columns. Raw item fields remain limited
to the requested projection. Context-aware CSV export includes that CIDR column
without replacing an optional name or dropping selected fields.

Creation and import acceptance use the reviewed application revision. Repeating
the same request at that revision recovers its bounded outcome while retained;
expired outcomes do not permit an old revision to create the resources again.
No raw request history or automatic address/version stream is stored.
Allocate IP / prefix is a separate action in an address space or prefix;
Create address space returns without losing the allocation draft.
An explicit prefix container supplies its declared namespace and enforces its
range. It does not supply routing or an interface configuration mask.
Editing an allocation from a focused scope explicitly reviews a context containing
its previous and proposed network references. Browsing scope remains unchanged.
The network-settings operation replaces the configuration and requires read
access to all its stored settings; absent optional settings need no read grant.
Projected omissions cannot implicitly clear hidden
VRF or L2 references. Use the resource field editor for an independent name change.
An explicit Prefix label removal can also accompany a network-settings edit in
the same conditional transaction. An untouched empty name preserves the label;
the `clear_name` input requests removal and requires permission to change it.
Participant assignment can explicitly search across permitted inventory from an
IPAM scope. Before committing, review the allocation, its shared group when
applicable, and the selected interface; browsing scope and permissions stay unchanged.
Removing a participant uses the same explicit context review and keeps both resources.

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

Saved views are personal initially. Creation and edits enforce a 512 KiB
aggregate metadata reply budget, leaving accepted views unchanged on refusal.
Team publication separately previews the definition disclosed, including roots
and predicates. Definition audiences and editors do not grant access to matching
inventory. Personal URL adjustments do
not modify the shared definition. Reset reloads that definition. Unavailable,
retired or retyped predicates fail explicitly instead of broadening the query.
Retired definitions keep their current values. Definition cleanup returns a
continuation even when a page contains only retained definitions; the host
resumes that cursor and safely restarts the pass after a process restart.
Skipping retained definitions creates no durable cursor or idle writes.

Network allocation distinguishes address space, L2 domain and routing context.
Requests explicitly select exclusive, VRRP or anycast use; omitted sharing is
unspecified and cannot convert a shared address into an exclusive allocation.
Selecting an existing group uses unspecified sharing and its reviewed revision;
the server derives the mechanism from that group.
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
Resources with retained IPAM state or attached relations cannot be archived
through the generic asset writer. Release relations explicitly before archival;
IPAM removal owns reference checks and allocation release.

## Shared L2 segments

Select the VMs you know share a segment and choose **Record shared segment**.
Choose an existing permitted L2 domain or create a distinct unnamed one. A name,
interface, IP, prefix, VLAN or routing context is not required to record this
knowledge. Matching names, CIDRs and VLAN IDs never merge domains automatically.
Each VM keeps its identity, placement and other network memberships.

The final summary records one attributed `On` relation for each participation.
With an unknown interface the source is the VM. Refining an explicitly selected
assertion changes its source to a known interface of that VM, preserving its
relation ID and checking current ownership, authority and revision. Interfaces of
a nested VM are not interfaces of its enclosing VM. No interface or physical
connection is invented; membership is not evidence of forwarding or reachability.
Repeating a coarser claim preserves its unique known interface without another
write. Multiple existing participations require an explicit interface or assertion
choice; omission cannot silently collapse them or append a redundant unknown one.

VM and L2-domain cards expose **Show memberships**, a bounded selected relationship
projection over these same relation identities. It pages authorized assertions,
not an estate-wide topology. VM reads follow only their owned network-bearing
layers, with a 1,024-node work ceiling, never another VM or a lateral gateway.
Continuations expire when inventory or authority changes. Read-only projections
write nothing. Mutation retries retain the reviewed command and use the common
finite outcome custody; unchanged accepted input creates no inventory history.
For segment saves, a canonical gRPC error may carry the
`omloom-mutation-outcome: not-committed` metadata only when that invocation
stopped before commit, outside saved-outcome disclosure. It does not settle an
earlier attempt. A rejected first save keeps the editable draft; after a lost
reply, retain and retry the exact reviewed command even if a retry is rejected.
Commit failures and failures while disclosing saved outcomes do not carry this
confirmation. Failed membership loads discard the previous page and continuation;
Refresh reloads the first current authorized page.
Record/refinement controls use current relation grants for the exact VM,
interface and segment, not the workspace-wide write indicator. Page context and
prepared actions include subject-side eligibility; each membership carries its
refinement eligibility, including manual source authority. These transient hints
do not grant access and are rechecked during preview and save.

Two explicitly selected L2 domains can use **Compare identity** and the existing
reviewed correction workflow. Participation transfers require explicit review;
unresolved operations, reservations, privileged bindings, conflicting sources or
canonical IPAM references must be resolved before merging. Grants, secrets and
provider authority never transfer. The ordinary bounded correction receipt and
undo rules apply, without a separate segment history.

CSV L2-domain rows can map `participants` in addition to optional `name`.
Use semicolon-separated qualified VM references `asset-id/incarnation`, optionally
followed by `@interface-asset-id/interface-incarnation` for a known interface and
`#relation-id` to refine one selected assertion. For example, a row's cell may
contain `VM_ASSET/VM_INCARNATION;SECOND_VM_ASSET/SECOND_VM_INCARNATION`, replacing
each placeholder with its actual UUID. At most 127 unique VMs and 32 KiB of input
are accepted per cell. The transport-only cell is decoded into the same command,
not retained as an inventory field. Preview and acceptance use the same atomic
validation; reimport preserves the source's domain and relation identities.

## CSV and storage bounds

An import names a personal source and explicitly maps selected columns. Each
row requires `external_id`; repeats update that source identity rather than
creating name-based duplicates. Omitted columns preserve fields; `<clear>` is
an explicit clear. Invalid rows identify the failing row and mapped column without
echoing its value or persisting a rejected preview; invalid input cannot be
partially accepted. A valid preview expires, and applying it rechecks the current
application revision, actor scope and field types. Raw CSV history is not saved.
Identity correction does not retarget personal CSV approvals. A minimal binding
to the retired identity prevents an old source from silently updating the retained
resource or recreating a duplicate; reversing the correction restores that binding.
Preparing a replacement atomically reclaims that importer's earlier review-only
drafts and their rows. Failed preparation preserves the prior draft; another
importer's previews remain under their own expiry and revision checks.
Known address CSV fields accept the same IP/CIDR grammar and explicit prefix
selection or creation. Prefix rows accept `cidr`, optional `name` and optional
`address_space`. Preview validates these through the same transaction owner and
acceptance atomically applies the reviewed batch. Reimporting an unchanged source
does not create another prefix, even when `prefix_name` is supplied. For an
existing source row, `create_prefix` preserves a matching declared membership
and its refined namespace and name; it never searches the estate for equal CIDRs.
Empty optional `prefix`, `prefix_name`, `broadcast` and `create_prefix` CSV cells
are omitted, allowing rows with different amounts of known information in one batch.
The name option applies to a newly created prefix. A direct new-prefix command
creates an independent identity instead. Canonical IP allocations, VLANs and shared groups
use the allocation form with typed scopes and uniqueness; generic CSV does not
promise those creation modes.

Export applies the current object and field projection before counts and output.
Spreadsheet CSV prefixes dangerous text with an apostrophe, including leading
tab, CR/LF and full-width formula characters. Typed integers and flags retain
their values. The typed API can request lossless machine CSV separately.
Spreadsheet editors can alter escaping on resave; these are different output
contracts.

Fixed operation ceilings are 100 resources per query page, 128 explicit targets
or CSV rows per mutation, 32 requested fields, 8 equality predicates, 256 KiB CSV
and one pending preview per actor. A larger operation must be narrowed rather
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

## Software and workloads

Use **Services** to record infrastructure execution contexts and adopted workloads.
OS/firmware installations locate services and logical interfaces; workloads locate
their current containers and responsibility. Product/release definitions are optional
references for selected runtime metadata. Open a resource to create its permitted local
components. A product contains release definitions. A machine keeps its physical
identity when Windows and Linux are recorded as separate installations; services
and logical interfaces belong to the corresponding installation. A firmware
installation belongs to its actual device or component. Changing the booted OS
does not remove the other installation.

The ordinary field editor, reviewed CSV import, selected columns, typed filters
and CSV export use these built-in current facts:

| Record | Fields |
| --- | --- |
| Product | `publisher`, `raw_name`, `normalization_basis` |
| Release | `version`, `raw_version`, `edition`, `architecture`, `artifact_digest`, `normalization_basis` |
| Installation | `installation_kind`, `installation_scope` |
| Workload | `workload_kind`, `compose_project`, `compose_service`, `compose_files`, `working_directory`, `repository_reference`, `source_revision`, `dockerfile_path`, `invocation_reference`, `image_digest` |
| Container | `runtime_object_id`, `runtime_lifetime`, `image_digest` |

Optional facts remain unknown when omitted; clearing is explicit. Preserve source
labels separately from reviewed normalized labels. `normalization_basis` names
the reviewed source and mapping revision, not an automatic matching rule.
`compose_files` is an ordered list of at most eight path references, one per line.
Repository and invocation references exclude credentials, query strings and
fragments. Image/artifact digests use `sha256:` and 64 hexadecimal digits. These
records do not store environment dumps, arbitrary command lines or repositories.
Unknown original invocation is valid.

In **Connections in broader scope**, link a release to its product using
**release of**, and an installation to its current release using **installed
release**. A release can have one product; an installation has one current
release. Update the installation binding for an upgrade, preserving its identity.
A referenced release's version, edition, architecture and artifact digest
cannot be rewritten into another release. Reusable definitions remain separate
from installations and do not prove that software is present or running.

Version labels are opaque current facts and can be unknown. No product, release
or version is required to record a known service/interface relationship. Version
interpretation belongs to a qualified adapter for a concrete infrastructure
capability. The inventory does not rank arbitrary releases, recommend updates,
collect every installed package or assess vulnerabilities and license compliance.

A workload represents a standalone or Compose deployment. Its **contains** links
identify owned containers and services; **runs on** links select the actual runtime
installation. Equal Compose names, image digests, IDs in different runtimes and
published ports never merge workloads. A recorded container ID/lifetime cannot
be rewritten into a replacement. Owner/operator use the common responsibility
card and may inherit through owned components, independently of creator
attribution and beneficiaries. Creator attribution is explicit and never inherits;
the person entering inventory is not automatically the deployment's creator.
Provider accounts may separately bind workloads and containers as consumers.
Runtime placement and catalog links do not transfer responsibility or access.
The Workloads category groups the current page by its permitted claimed owners.
Owner badges filter by the exact party identity; the resulting selection can be
saved personally or published through the ordinary view workflow. Unknown,
explicitly unassigned and restricted ownership remain distinct. Only the visible
page receives a bounded owner projection, without contacts or source-resource
identities. It is calculated through the common resolver and is not another
persisted inventory or a grant.

Machines and installations also accept `purpose`, `supplier`, `model`,
`purchase_reference`, `contract_reference`, `lifecycle`, `lifecycle_date`,
`warranty_until`, `support_until` and `disposal_reference`. Dates use `YYYY-MM-DD`.
Lifecycle enum values distinguish planned, received, installed, in-use, retired
and disposed inventory; they are independent of power, health and verified data
erasure. Workloads and containers may have their own lifecycle state. Commercial
and source-path fields retain individual field rights. Contract references contain
no license keys and do not establish legal compliance or schedule external work.

Facts use the same transactional field authority, edit revision and audit as
other inventory. Unchanged values do not create payload versions; reads do not
write a comparison or access history. Adapters and execution capabilities remain
separately qualified. The development fixture supplies no production identity.

## Native host reports

Build and run the one-shot collector on the machine being observed:

```sh
cargo build --locked -p omloom-observe
cargo run --locked -p omloom-observe -- --output host.omloom
cargo run --locked -p omloom-observe -- --check host.omloom
```

The output path must be new. Publication uses a synced temporary file and an
atomic no-replace operation; an interrupted collection cannot publish a partial
frame. Reports contain private host facts. Keep them outside version control.
The check command prints coverage and counts, without inventory names or IPs.

External commands have a ten-second deadline within the ninety-second collection
budget. A timed-out child is terminated and reaped; diagnostics identify the
executable and, for Windows PowerShell sources, the module and native cmdlets,
without dumping scripts or their output. Timeouts leave coverage unavailable.
Optional source failures remain in the report's coverage. Permanent hardware and
installation identifiers are optional; their absence produces partial identity
coverage, not an invented fingerprint. The collector still checks boot consistency
before publishing. If required boot or interface evidence cannot be collected, the command reports collection
unavailable and exits successfully without writing a report or replacing an
existing file. This is not a verified inventory or an empty scan. Invalid
arguments, malformed report frames and file publication errors remain errors.

In the workspace, choose **Import host report**, supply a source name and select
the resources to adopt. Select required parents together with their components.
Match an existing resource only after checking its identity and incarnation.
Names, matching addresses, process names and Compose labels do not establish
continuity, ownership or permission. Review the proposed changes before acceptance.
The same flow works on a phone, including selecting a report from its file picker.

### Reports without permanent identifiers

An empty `HostReport.source_basis` means continuity is unavailable. Each selected
finding must be explicitly matched or separately adopted. A later package starts
unresolved even if its filename, boot, name, addresses or configuration are equal.
An unselected finding creates no permanent unknown resource. Current manually
confirmed source custody is keyed by the selected resource incarnation, rather
than creating a new binding for every package. Retry an uncertain acceptance with
the same preview receipt, as described below.

**Compare possible matches** queries the current authorized working context.
Results show same-layer supporting evidence, differences and unknowns, with a
revision-bound continuation for further candidates. Select up to two candidates
to compare against the report before confirming an exact resource. Similar fleet
configurations remain ambiguous. A shared installation UUID can be cloned; a
factory MAC concerns an interface, not the chassis. There is no probability score,
automatic first-result choice, permission transfer or implied remote access.
Use manual search or leave the finding unresolved when the available evidence
cannot distinguish machines. Duplicate accepted records still use the separate
identity-correction workflow.

Comparison and manual search return their inventory revision. Preparing an
import rechecks that basis; an intervening change preserves the draft but
requires another comparison. Candidate retrieval is paginated, with bounded
upward membership checks for selected roots and field authorization before
comparison details are decoded. Exhausting a work bound is an explicit error,
not evidence that no matching resource exists.

The native profile can additionally report OS-visible logical CPU count,
visible memory in bytes and a reported CPU model. These describe the installation's
execution context, not a unique host ID, physical installed RAM certification or
capacity reservation. Missing values remain absent. Selected current fields are
probed independently through the existing index; there is no combined fingerprint,
similarity table, candidate history or writes caused by opening a comparison.
Current field projections are rebuilt resumably on upgrade.

For a host already imported, open its device or VM card and choose **Load
observation package**. Select an existing compatible source and confirm that the
package describes this host and the selected installation. Source references are
server-owned; no source-name retyping or renaming is required. Only permitted,
already mapped resources can be selected. New findings require separate adoption.
If no compatible source can establish continuity, choose **Match this package
manually**. Confirm the existing host and each selected finding explicitly through
the same import workflow. This records current observations without renaming or
replacing an existing qualified source, and without adopting another chassis.
During anonymous refresh, candidate pages are checked against the chosen source's
existing custody and this host before choices are offered. This bounded read sends
only the selected source reference and candidate identities, not the report again.
Eligibility never confirms the match automatically or permits new adoption.
Source discovery considers at most 128 host bindings, each matching at most the
package's 128 findings, within the shared 512 KiB encoded reply ceiling. Exceeding
an aggregate reply bound returns a limit error rather than silently omitting sources.
A selected child's existing parents are checked without refreshing their fields.
The preview shows this source's observations and address differences; acceptance
preserves intended fields, independent sources and omitted partial evidence.
Equal values in a genuinely newer observation update only their freshness.
A file proves its collection instant, not a future health or connectivity lease.

Back retains the draft and selected observations. Acceptance retains the same
bounded mutation outcome as other reviewed imports. If a reply is lost, retry
the same acceptance; do not prepare a different change while its outcome is
unknown. A committed result can be recovered after restart without another
mutation. Expired previews cannot be applied as new work, and replay still
requires current read access to the result. Raw reports are not saved in browser
storage or retained as inventory history.

The source name selects an import configuration within your actor's namespace.
Reuse it exactly for repeat imports. A changed name selects an independent
source, rather than renaming a display label, and can propose new identities.
Equal report source evidence does not merge separate configurations; match
existing resources explicitly when that is intended.

The OS cannot prove whether its execution host is physical or virtual. For a new
host, explicitly choose **Physical device** or **Virtual machine**. Alternatively,
match the current device/VM scope or another reviewed existing resource. That
match preserves its accepted type, name and fields; hostname/model observations
are initialization proposals, not authority to rename existing hardware or VMs.

Routes, binds of selected services, and application-firewall observations each
require separate selection. None are enabled by default. Unselected findings and
sections are discarded before bounded preview storage. Acceptance rechecks scope,
field rights and revision, then commits the selected tree and source mappings
atomically. Previews expire; accepted observations reuse the current-only storage
and preserve omitted facts when a source or selection is partial. Importing an
identical accepted file creates no inventory versions or new payloads.
Replaying accepted proposals cannot undo subsequent local edits. Older reports
and conflicting reports at the same source instant are refused. Each selected
network section has its own current checkpoint, so accepting routes does not
implicitly accept firewall data or prevent reviewing it separately.

For selected interfaces, review lists added addresses, removed addresses from a
complete refresh, and changed attributes. The comparison concerns the selected
source only. A partial refresh preserves omitted addresses and their original
observation dates. It never changes another source, an intended assignment or a
service's literal bind. Current-address read rights apply even when an existing
preview is reused. Preview custody stores the selected input, not another copy
of the derived comparison or an address history.

Select two devices, VMs, OS installations or interfaces and choose **Compare
identity** to inspect their current permitted fields and source claims together.
Hardware and installation identifiers have separate types and namespaces;
matching reported digests do not establish authenticated identity or merge
resources. Comparison reads only the two selected subjects, without loading
their descendants or routing tables, and writes nothing.

Files are limited to 256 KiB, 128 findings and 2944 protobuf field visits before
decoding. The remaining request budget accommodates explicit matches and context;
small files with excessive nested fields are refused before generated allocations.

Native modules use selected OS interface, routing and socket sources. macOS reads
ifconfig, registered hardware ports, Thunderbolt receptacles, lsof, netstat, ALF
and PF coverage. Linux uses iproute2, ss and nftables coverage. Windows uses native
network CIM/PowerShell sources and ActiveStore profiles. Missing privileges and
tools remain explicit. An empty kernel command does not prove that no listeners
exist. Kernel binds without a qualified process owner remain report observations,
not newly invented services. Sequential source reads are not an atomic host snapshot.
Application rules, route presence and wildcard binds do not prove effective access.

Linux interfaces without an IP can still represent L2 bridges, veth links and
tunnels. They are not discarded merely for lacking an address. The bounded
collector refuses an oversized source or report explicitly, without publishing
a truncated inventory. Interface failures include the underlying limit or
command diagnostic. Dense hosts that exceed these limits need a narrower
collection scope; the current one-shot command has no interface selector.

Temporary IPv6 literals are excluded, including matching bind and route literals.
Preferred/deprecated validity remains separate from generation class. Windows
random-origin IPv6 is conservatively excluded where its public/temporary class
cannot be established; that address result remains partial. Loopback and link-local
addresses are not ordinary interface inventory. The collector never stores a Wi-Fi
MAC as device identity.

Optional local Docker and NetBird modules do not scan arbitrary software packages.
Docker candidates distinguish runtime, workload, container lifetime and endpoint
attachments. Docker Desktop guest addresses are not macOS host interfaces.
Selected containers are inspected in one bounded batch. Missing, repeated or
unexpected identities refuse that batch; only consumed container and attachment
fields are requested. Windows socket collection avoids repeated array copies.
The selected context or environment endpoint is resolved once using Docker's
context precedence. All daemon reads pin that checked local Unix socket or
Windows named pipe; remote endpoints are refused. Daemon identity is rechecked
before accepting the batch. Module diagnostics name the failing stage without
retaining raw stderr or connection settings.
EXPOSE, a published port or a Compose label cannot prove a listener, reachability,
human owner or recoverable launch invocation. NetBird attribution uses local daemon
IP evidence and a unique observed interface match; it retains no peers or keys.
Reports contain no environment, full commands, credentials or installed-package
inventory. There is no implicit elevation, remote Docker connection, SSH delivery
or resident installation.

## Reviewed identity correction

Select two devices, VMs, OS installations or interfaces and choose **Compare
identity**. Matching reported identifiers support review but do not prove identity
or authorize a connection. Candidate lookup is also available before adopting
report findings, within the current permitted selection. Missing evidence,
cloned UUIDs, movable interfaces and reused addresses never trigger an automatic
merge or an access grant.

Choose whether the records are separate, one chassis with distinct OS installs,
or duplicates of the same installation. Choose the retained identity and every
field resolution. Add only the OS/interface mappings actually established as
duplicates. Separate boots retain their own services, interfaces and current
addresses. Each physical attachment needs individual confirmation, followed by
ordinary containment, exclusivity and cycle validation.

Native sources keep independent bindings even when their display labels match.
Current field claims move with their original source IDs and individual freshness
windows, separately from intended-field choices. Moving or reversing them requires
read and write access to each affected field on both identities. Conflicting claims
at the same source/field key are refused rather than silently overwritten.
Manual or unresolved legacy namespace collisions require a reviewed rename.
Neither source relocation nor an address change rewrites literal service binds,
routes, firewall policy or VLAN/VRF membership. No grants, secrets, account control
or operation targets transfer. Explicit responsibility and external-effect
conflicts must be resolved first.

The bounded preview changes no inventory. Accepting it checks the same working
context, current authority and installation revision, then commits atomically.
Its receipt supports an exact retry and a finite reversal window. Reversal
requires current rights and no subsequent accepted mutation; otherwise prepare a
fresh correction. Old references resolve read-only to the canonical incarnation
only when both original and canonical rights permit it. They never become write
or execution aliases.

## Inspect current infrastructure in 3D

Choose **3D scene** in Hardware or Overview (inside **More** on a phone), then
choose an enclosure from the current selection. The scene uses current accepted
physical definitions, placement envelopes and port ownership. **Show components**
reads one page of that resource's local children; **More components** explicitly
continues it. The scene admits at most 128 selected resources and 256 selected
bindings. It never loads an entire site just to draw one rack.
Automatic expansion requests `Query.require_physical_read`, applying current
physical-field permission before count and pagination. Ordinary inventory can
still show a permitted label without geometry; an explicit unauthorized scene
subject remains an error rather than being silently redacted.

Rotate, zoom and pan the view, select an object or use the equivalent resource
list, then **Open resource** for its ordinary card. **Return to table** preserves
the inventory journey. If graphics are unavailable or the context is lost, the
resource list remains available. A viewport test does not qualify a native phone
GPU or webview.

Only recorded geometry is drawn. Unknown or ambiguous positions stay in the list;
the renderer does not assign plausible dimensions or invent empty space. The
selected enclosure and bodies whose measured children have been expanded use
a labeled translucent cutaway. Their surfaces do not intercept child selection;
the enclosing resources remain selectable in the list. Closed bodies remain
opaque. This is an occlusion treatment, not a confidence score.

`Physical.scene` reprojects explicit current incarnations in one read snapshot.
Current named cable ends may add up to 64 independently authorized cable labels,
but no remote unselected port. A dashed straight line requires exactly two visible,
located actual ends. It is a connection diagram, not a surveyed route, fit proof
or service-health claim. Breakout assemblies are not paired by guesswork.

`Physical.port_anchor` records a measured port envelope against a reviewed
`HasPort` binding in its owner's coordinate frame. It uses the same current-pose
store as installed geometry, checks both objects' physical rights and the current
binding, and leaves ownership and connectivity unchanged. Repeating unchanged
geometry from the same reporter adds no record or audit entry. Removing or moving the binding removes
its old pose. Missing coordinates remain unknown.

On a phone, use **Select** to show row checkboxes, then **Cancel selection** to
clear selection and reclaim the column. Tablet lists keep them visible. Desktop
lists reveal them on row hover, keyboard focus or while a selection is active.

## Record a VM's new hypervisor

After migrating a VM outside Omloom, open its details and choose **Change
hypervisor after migration**. Drag the VM card onto an enabled destination on
desktop, or use **Choose** on a phone or keyboard. Destination cards show
permitted hardware paths, addresses and exact identities. Searching and paging
stay within the selected enclosing context; choosing all permitted infrastructure
is an explicit action.

Review the actual old and new host, then **Record and view VM**. This records
inventory placement only: it does not invoke a hypervisor migration. The same
VM, OS installations, interfaces, services and source mappings remain in place.
Current IPs can stay unchanged or be updated by a separate guest observation.
Service binds, routes, network/storage membership and grants are not inferred
from the destination.

One VM has one accepted current hosting placement. Recording a move conditionally
updates that relation, rather than adding another host. A hidden old host, an
obsolete review, an ineligible destination, a cycle or unresolved guest effects
prevent acceptance. Ambiguous existing multi-host records require explicit
resolution through **Connections in broader scope**; upgrading never guesses
which host to retain.

Direct ownership stays unchanged. The review identifies permitted inherited
facets bound to the old placement; those become source-unavailable after its
revision changes. Review their administrative source separately in the existing
responsibility editor. The new host never silently becomes the owner.

## Physical plans and cable ends

Physical plans use the same accepted inventory, revision checks and requirement
evaluation as ordinary resource work. Saving a draft does not reserve capacity
or report installation. Reservation and attributed manual completion are separate
commands. Completed effects survive cancellation of remaining work.

Reservation requests must fit the host's configured `physical_reservation_max_ms`
duration. The development host sets a 24-hour ceiling. This new-admission setting
does not change the durable quota/retention identity or invalidate accepted holds.
An expired deadline alone does not prove reserved work was not performed;
unresolved holds require explicit reconciliation before reuse.

A required postcondition can remain unknown after work is reported; an old observation is
not automatically evidence of the new result.

Recorded power-path separation is another condition in that same evaluator.
Select two feeds and, when known, their terminal boundaries. A visible shared
UPS is a counterexample without needing to read its private upstream supplies.
A positive result requires explicitly complete source lists through the selected
boundaries. Missing boundaries allow counterexamples only; an early boundary
cannot hide known dependencies. Record current source-list coverage in physical
facts and use ordinary Powered by relationships for the actual topology.
These checks do not certify live power, electrical safety or surviving capacity.
The same condition can be saved as a physical step's postcondition.

Rack elevations project the recorded enclosure frame. Rear viewing mirrors the
horizontal axis and orders reported bodies from far to near; proposed placements
remain explicit overlays. Switching faces changes no inventory coordinates.
The reported coordinate table keeps exact values available, including geometry
that cannot be drawn, in pages of 25 without loading neighboring infrastructure.
Installed parts are fetched one server page at a time. Continuations are bound
to the enclosure, working scope, permissions and inventory revision. A page is
not a claim of complete occupancy or free capacity; admission checks all relevant
current occupants and reservations independently of the displayed page.
An optional sourced rail grid records its top Y coordinate, unit count, lowest
printed number and numbering direction on the current rack profile. Cabinet
height does not supply these facts. Printed U selection follows this declaration
and rounds allocation height outward to half-U increments without changing the
equipment dimensions. Without the declaration, the geometric offset editor and
explicit coordinate drafts remain available and do not claim printed rack units.
Space reuse after a removal requires the corresponding prerequisite in the common
plan. A future removal does not free current occupancy for an independent step.
Uncertain overlap between old and new loads keeps capacity unqualified. Manual
reports record work already performed, including discrepancies, and do not
certify physical fit or dispatch another operation.
An enclosure replacement can include an explicitly reviewed incoming allocation
on that same step. Reporting it commits the new binding and its reported pose
atomically. The previous allocation can be selected as a proposal for review;
it does not supply the incoming dimensions, cable attachment or physical proof.

Named ends belong to a retained cable. Their definition, accepted destination
intent and reported attachment are distinct. A confirmed loose end releases its
physical binding while preserving intent unless explicitly cleared. An unknown
attachment retains possible occupancy and cannot make that socket available to
another cable. Replacing a component never transfers its cable attachment or
service evidence to the replacement by position or name.

Cable-end transitions are ordinary steps in a physical plan. Repeated work on
the same end references the earlier step's recorded result. Conflicting socket
claims require a preceding release through the shared dependency graph; a plan
cannot promise a free socket to two ends at once. Retries and reads do not append
connection histories. Endpoint visibility and relationship rights are checked
before displaying or changing the attachment.

One DAC or AOC is one adopted cable assembly, including inseparable end modules.
Record only its real named ends and known internal channel numbers. Breakout
legs share that assembly identity; each declared channel has at most two ends.
Missing channel data is unknown. These declarations do not certify polarity,
speed, signal continuity or service health.

Replacement plans retain explicit endpoint judgments separately from the
component event. Unmapped ports remain visible when a smaller replacement has
fewer ports. Whole-port and individual-channel allocations cannot overlap;
accepted breakout lanes require a current sourced port-channel declaration.
Correspondence does not transfer configuration, grants, credentials or wiring.
An exact-binding postcondition uses the common requirement evaluator and fails
when its reviewed relation is removed, recreated or changed.

The replacement review explains its current evidence using versioned rules and
strength labels, without invented probabilities. Confirm, reject and defer are
attributed judgments, not installation reports. Current decisions contribute
once to fixed-window local quality counts; correcting a decision replaces its
contribution atomically. Deferral is unlabelled, not a negative example. Counts
are available under the complete contributing read-grant and working scope,
including read-only auditors. Changing authority or field access invalidates
that projection.
These are workspace review counts, not counts for the currently opened enclosure.
Opening another resource does not create another aggregate partition.
The configured detail-retention period bounds the correction/count window;
expired counts use ordinary bounded cleanup. Minimal current disposition stays
with active plan work to prevent recounting unchanged evidence, and terminal
plan detail follows its existing lifecycle. No raw training corpus or automatic
retraining is created. Acceptance counts do not establish accuracy or recall.

An optional sourced mating contract identifies one exact declared interface and
its complementary sides. Matching connector labels alone cannot establish fit;
different contract revisions remain unqualified. The declaration checks only
that interface, not signal compatibility, power safety or working service.
Normal reserved cable completion rechecks the current endpoint declarations.
Explicit actual-state reporting can retain an incompatible connection without
asserting physical fit or service health.

Equipment may also declare six service/connector margins in its enclosing
coordinate frame. A missing declaration is unqualified, while explicit zero is
a supplied measurement. Admission checks material bodies against neighboring
declared service spaces and the candidate's service space against other bodies.
Empty service spaces may overlap each other. An interior envelope does not
certify an outside maintenance aisle or a cable route.

A plan can retain up to 64 material requirements, each with a scalar positive
quantity and optional sourced equipment claims or physical specifications. These
are plan-local intentions, not assets, orders or promised stock. Even a large
quantity creates no rows per unreceived unit. Current occupancy and power-source
completeness cannot be asserted for proposed equipment.

A selected requested unit can be placed and reserve space before receipt, using
its declared dimensions and demand. The placement references its line and unit,
not a fabricated inventory ID. Other plans account for that reserved geometry,
clearance and load. Receipt supplies the real adopted incarnation to the same
step; installation cannot be recorded while that unit remains unreceived.

Explicit receipt or reuse links an adopted resource to one requested unit.
For a saved draft, reserved or partially completed plan, **Create received device**
uses the ordinary inventory editor and commits one new Device, actual entered
fields, the exact unit assignment and any required hold together. The material
label is an editable name suggestion; serial and measured characteristics may
remain unknown. Planned specifications never become actual equipment facts.
The creation context is explicitly permitted installation inventory. No rack
attachment, move, installation completion, warehouse receipt or verified fit is
implied. The recorded assertion includes the server actor and recording time,
not an invented delivery or inspection date.

**Record and next** independently confirms each item and clears its serial and
actual characteristics. A conflict leaves earlier successes intact. Allocated
resources are exact action operands, not additional
recursive browsing roots. An allocated enclosure does not admit its siblings
or descendants outside the selected viewing context. Before
submission the browser keeps a non-authorizing plan, material, unit and revision
reference. Lost replies and reopening trigger automatic recovery. Transactional
browser storage admits one unresolved save per exact material unit across tabs,
with at most 128 unresolved references. Other units and plans remain usable.
Checks of another unit only retrieve an already accepted outcome; they cannot
close its admission while its save may still be in flight.
Generated keys prevent delayed cleanup from removing another attempt's reference;
no actual fields or request payload are stored. Storage failure prevents dispatch.
Lookup and exact replay write nothing. The recovery command can close an unused
admission revision through the existing root CAS, without creating a device or
another outcome ledger. Changed arguments cannot reuse the attempt.
Expired outcomes do not admit the old attempt as fresh creation, and
restored stores retain their existing mutation fence. An unavailable result
remains unresolved for that unit, never an automatic inventory-only fallback.
**Find existing device** uses the ordinary inventory picker and assignment command
to resolve the physical item without creating it again. There is no separate
manual approval process. The browser removes that unit's reference only after
known recovery, definitive non-acceptance or confirmed existing-device assignment.
Original accepted
revision and current plan allocation/hold state are separate meanings.
After a definitive rejection, the form retains actual input and offers a separate
**Create in inventory only instead** action. That action does not receive or assign
a plan unit. **Review current receipt** explicitly refreshes the saved material
unit and inventory creation context without clearing entered actual fields.
Receipt application preserves unrelated draft edits rather than saving them.
Changed material lines require explicit reconciliation against the saved plan.
A delivery reference carries to the next item only when explicitly
selected; serials and measured facts never carry over.

Receipt may be partial and can continue after reservation. It neither installs
the resource nor proves compatibility or health. Real allocations use the same
serialized-part locks as plan steps.
Normal completion rechecks a received unit's actual dimensions and demands through
the common physical admission mechanism, excluding only the hold being consumed.
An explicit actual-state report remains available for work already performed;
it does not establish compatibility or healthy service.
Cancellation releases unused materials but preserves holds for unresolved effects.
An allocation already used by a step
cannot be silently replaced. Terminal BOM detail follows the existing finite
cleanup lifecycle. Equal input, including equivalent quantity spellings, writes
no additional revision or history.

Selected serialized materials combine step targets and real BOM allocations.
Moving and placing the same retained component counts it once. The list separates related
reported work from outstanding reservations and includes a serial only when
that current field is permitted. Export reloads the authorized plan first; it
does not order equipment, copy credentials or retain an inventory snapshot.

An explicit availability profile selects an enclosure region, recipient and
permitted capacity constraints. Its recipient also needs current availability
permission; knowing a profile ID is insufficient. The response contains that
region's result without occupant names, counts or silhouettes. Revocation stops
new disclosure, and restored data cannot certify fresh availability. This is
advisory information, separate from atomic reservation.

Usable dimensions do not prove empty space. Record the bounded region whose
current occupants are completely represented before qualifying its availability.
Outside that region remains unknown. Whole-enclosure mass, power and cooling
checks need coverage of their contributing area. Withdrawing this statement
releases neither installed occupancy nor unresolved reservations. Drafts and
attributed reports of actual work remain available without a complete survey.

## Verification

```sh
sh scripts/check-contracts.sh
```

This checks the real API/store and starts the compiled host for Chromium
scenarios with temporary disk directories. It does not intercept API requests
or replace the database. The scripts own their temporary data and generated
verification output. Native OS evidence and production identity integration
remain separate qualifications.
