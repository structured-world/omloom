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
Ordinary text and bay counts can also be entered directly in an import. Preview
and ordinary field validation apply before accepting imported claims.

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
New IPs, prefixes, VLANs and shared groups use the network allocation form,
which checks their typed scopes and uniqueness. The generic CSV preview rejects
these creation modes instead of promising a change that cannot be accepted.

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

In the workspace, choose **Import host report**, supply a source name and select
the resources to adopt. Select required parents together with their components.
Match an existing resource only after checking its identity and incarnation.
Names, matching addresses, process names and Compose labels do not establish
continuity, ownership or permission. Review the proposed changes before acceptance.
The same flow works on a phone, including selecting a report from its file picker.

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

## Verification

```sh
sh scripts/check-contracts.sh
```

This checks the real API/store and starts the compiled host for Chromium
scenarios with temporary disk directories. It does not intercept API requests
or replace the database. The scripts own their temporary data and generated
verification output. Native OS evidence and production identity integration
remain separate qualifications.
