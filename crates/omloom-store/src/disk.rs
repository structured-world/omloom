use crate::{CLASSES, Class, Policy};
use coordinode_core::{
    graph::{node::NodeId, types::Value},
    schema::definition::{ConstraintKind, EdgeTypeSchema, LabelSchema, PropertyDef, PropertyType},
};
use coordinode_embed::{
    Database, DatabaseError, EdgeTypeDeclaration, Intent, LabelDeclaration, PackageConstraint,
    SchemaPackage, TransactionId,
};
use omloom_contracts::{
    Record,
    ids::{AssetRef, OperationId},
    wire::{self, EffectOutcome, OperationState, field, record},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::Duration,
};

type Properties = BTreeMap<String, Value>;
type Result<T> = std::result::Result<T, Error>;
const ROOT: &str = "installation";
const SCHEMA_VERSION: u64 = 7;
const MAX_CHILDREN: usize = 2048;
mod inventory;
mod lifecycle;
mod operations;
mod scans;
#[cfg(test)]
mod tests;
pub mod workspace;
pub use lifecycle::{ActivityEntry, Cleanup};
pub use scans::ScanState;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Export(Box<coordinode_embed::backup::export::ExportError>),
    Restore(Box<coordinode_embed::backup::restore::RestoreError>),
    Database(Box<DatabaseError>),
    Invalid,
    Conflict,
    Missing,
    Limit,
    Corrupt,
    SchemaVersion,
    PolicyChanged,
    Expired,
    RecoveryRequired,
    UnsafeTransition,
    Denied,
    Incomplete,
}
impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<DatabaseError> for Error {
    fn from(e: DatabaseError) -> Self {
        match e {
            DatabaseError::RevisionMismatch { .. }
            | DatabaseError::UniqueViolation { .. }
            | DatabaseError::TransactionConflict { .. } => Self::Conflict,
            DatabaseError::Budget(coordinode_core::budget::BudgetStop::Memory { .. }) => {
                Self::Limit
            }
            other => Self::Database(Box::new(other)),
        }
    }
}

/// An owned database namespace. Open once per process, share by reference/Arc.
/// Concurrent hosts must share a supported replicated Database, not independently
/// open the same directory. All exclusion and counters are database transactions.
pub struct Store {
    db: Database,
    policy: Policy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetSummary {
    pub target: AssetRef,
    pub kind: wire::AssetKind,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub revision: u64,
    pub assets: Vec<AssetSummary>,
    pub next: Option<String>,
}

/// Host-validated operation intent. Arguments are bounded domain values, never
/// credentials. Caller identity is an opaque reference from the admitted session.
/// Arguments include the action's confirmed non-secret recovery and authority
/// references. The store never interprets a reference as authorization.
pub struct Plan<'a> {
    pub target: AssetRef,
    pub inventory_revision: u64,
    pub caller: &'a str,
    pub action: &'a str,
    pub arguments: &'a [wire::Field],
    pub retry_key: &'a str,
    pub expires_at_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub operation: OperationId,
    pub worker: String,
    pub epoch: u64,
    pub until_ms: u64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DurableOperation {
    pub id: OperationId,
    pub target: AssetRef,
    pub state: OperationState,
    pub outcome: EffectOutcome,
    pub revision: u64,
    pub provider_task: Option<String>,
    pub action: String,
    pub caller: String,
    pub inventory_revision: u64,
    pub plan_digest: [u8; 32],
    pub arguments: Vec<wire::Field>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    /// Charged rows, including capacity reserved for pending settlement.
    pub rows: [u64; CLASSES],
    /// Charged logical bytes, including settlement and lease-owner headroom.
    pub bytes: [u64; CLASSES],
    pub revision: u64,
}

fn text(s: impl Into<String>) -> Value {
    Value::String(s.into())
}
fn int(n: u64) -> Result<Value> {
    Ok(Value::Int(i64::try_from(n).map_err(|_| Error::Invalid)?))
}
fn string(p: &Properties, k: &str) -> Result<String> {
    p.get(k)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(Error::Corrupt)
}
fn number(p: &Properties, k: &str) -> Result<u64> {
    p.get(k)
        .and_then(Value::as_int)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(Error::Corrupt)
}
fn params(p: impl IntoIterator<Item = (&'static str, Value)>) -> Option<HashMap<String, Value>> {
    Some(p.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
fn props(items: impl IntoIterator<Item = (&'static str, Value)>) -> Properties {
    items.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

fn charged_row(key: &str, parent: &str, class: Class, mut data: Properties) -> Result<Properties> {
    data.insert("key".into(), text(key));
    data.insert("parent".into(), text(parent));
    data.insert("class".into(), int(class as u64)?);
    let mut size = 128u64;
    for (name, value) in &data {
        let bytes = match value {
            // A recovery owner may have a longer identifier than the first
            // worker. Charge its full supported width before any dispatch.
            Value::String(_) if parent == "leases" && name == "worker" => 256,
            Value::String(value) => value.len() as u64,
            Value::Binary(value) => value.len() as u64,
            _ => 16,
        };
        size = size
            .checked_add(name.len() as u64 + 64 + bytes)
            .ok_or(Error::Limit)?;
    }
    if parent == "relations" {
        size = size.checked_add(384).ok_or(Error::Limit)?;
    }
    data.insert("size".into(), int(size)?);
    Ok(data)
}

fn audit_data(actor: &str, action: &str, revision: u64, now: u64) -> Result<Properties> {
    Ok(props([
        ("actor", text(actor)),
        ("action", text(action)),
        ("revision", int(revision)?),
        ("at", int(now)?),
        ("pinned", Value::Bool(false)),
    ]))
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 15) as usize] as char);
    }
    encoded
}
fn hash_part(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}
fn hash_optional(hash: &mut Sha256, value: Option<&str>) {
    if let Some(value) = value {
        hash.update([1]);
        hash_part(hash, value.as_bytes());
    } else {
        hash.update([0]);
    }
}
fn hash_field(hash: &mut Sha256, value: &wire::Field) -> Result<()> {
    hash_part(hash, value.name.as_bytes());
    match value.value.as_ref().ok_or(Error::Invalid)? {
        field::Value::Text(value) => {
            hash.update([1]);
            hash_part(hash, value.as_bytes());
        }
        field::Value::Integer(value) => {
            hash.update([2]);
            hash.update(value.to_be_bytes());
        }
        field::Value::Flag(value) => hash.update([3, u8::from(*value)]),
        field::Value::Clear(0) => hash.update([4]),
        _ => return Err(Error::Invalid),
    }
    Ok(())
}
fn digest_value(value: [u8; 32]) -> Value {
    Value::Binary(value.to_vec())
}
fn stored_digest(p: &Properties) -> Result<[u8; 32]> {
    match p.get("digest") {
        Some(Value::Binary(value)) => value.as_slice().try_into().map_err(|_| Error::Corrupt),
        _ => Err(Error::Corrupt),
    }
}
fn token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn bounded(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 256 {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}
fn asset_key(target: AssetRef) -> String {
    format!("asset/{}", target.asset)
}
fn operation_key(id: OperationId) -> String {
    format!("operation/{id}")
}
fn target_props(target: AssetRef) -> Properties {
    props([
        ("asset", text(target.asset.to_string())),
        ("incarnation", text(target.incarnation.to_string())),
    ])
}
fn target(p: &Properties) -> Result<AssetRef> {
    Ok(AssetRef {
        asset: string(p, "asset")?.parse().map_err(|_| Error::Corrupt)?,
        incarnation: string(p, "incarnation")?
            .parse()
            .map_err(|_| Error::Corrupt)?,
    })
}

fn unpack(rows: Vec<Properties>) -> Result<Vec<Properties>> {
    rows.into_iter()
        .map(|mut row| match row.remove("p") {
            Some(Value::Map(p)) => Ok(p),
            _ => Err(Error::Corrupt),
        })
        .collect()
}

impl Store {
    pub fn open(path: &Path, policy: Policy) -> Result<Self> {
        Self::open_with_query_memory(path, policy, 32 * 1_048_576)
    }

    /// Opens the same durable store with an explicit host-selected statement
    /// memory ceiling. This is independent of admission and owned disk limits.
    pub fn open_with_query_memory(path: &Path, policy: Policy, bytes: u64) -> Result<Self> {
        if !policy.valid() {
            return Err(Error::Invalid);
        }
        let mut db = Database::open(path)?;
        db.set_max_interactive_txn_bytes(16 * 1_048_576);
        db.set_query_memory_limit(bytes)
            .map_err(|_| Error::Invalid)?;
        db.set_retention_window(Duration::from_secs(1));
        let store = Self::from_database(db, policy)?;
        if path
            .join("RESTORE_REQUIRED")
            .try_exists()
            .map_err(Error::Io)?
        {
            store.disable_restored_dispatch()?;
        }
        Ok(store)
    }

    /// Also accepts a host-owned replicated database using the same transaction API.
    pub fn from_database(mut db: Database, policy: Policy) -> Result<Self> {
        if !policy.valid() {
            return Err(Error::Invalid);
        }
        let mut schema = LabelSchema::new_node_id("OmRow");
        schema.add_property(PropertyDef::new("key", PropertyType::String).not_null());
        for name in ["candidate", "cursor", "snapshot"] {
            schema.add_property(PropertyDef::new(name, PropertyType::String));
        }
        schema.add_property(PropertyDef::new(
            "virtualization_role",
            PropertyType::String,
        ));
        for name in ["completeness", "review_rows", "review_bytes"] {
            schema.add_property(PropertyDef::new(name, PropertyType::Int));
        }
        for name in [
            "parent",
            "policy",
            "asset",
            "incarnation",
            "layer",
            "authority",
            "connection",
            "object_kind",
            "object_id",
            "lifetime",
            "field",
            "source",
            "claim",
            "source_revision",
            "scan",
            "operation",
            "caller",
            "action",
            "provider_task",
            "worker",
            "actor",
            "to_asset",
            "to_incarnation",
            "vtext",
            "name",
            "name_sort",
            "description",
            "search_term",
            "asset_key",
            "workspace_kind",
            "label",
            "help",
            "owner",
            "namespace",
            "cidr",
            "address_space",
            "link_scope",
            "routing_context",
            "sharing",
            "group",
            "address",
            "meaning",
            "zone",
            "binding",
            "ip_order",
        ] {
            schema.add_property(PropertyDef::new(name, PropertyType::String));
        }
        for name in [
            "schema",
            "revision",
            "class",
            "size",
            "kind",
            "adoption",
            "observed",
            "valid",
            "inventory_revision",
            "state",
            "outcome",
            "at",
            "expires",
            "epoch",
            "until",
            "vtype",
            "vint",
            "settlement_rows",
            "settlement_bytes",
            "definition_type",
            "generation",
            "vrid",
            "priority",
            "prefix_len",
            "family",
            "vlan_id",
            "prefix_length",
            "count",
            "bytes",
        ] {
            schema.add_property(PropertyDef::new(name, PropertyType::Int));
        }
        for name in [
            "restored",
            "pinned",
            "vflag",
            "settled",
            "searchable",
            "retired",
        ] {
            schema.add_property(PropertyDef::new(name, PropertyType::Bool));
        }
        for name in ["counts", "sizes"] {
            schema.add_property(PropertyDef::new(
                name,
                PropertyType::Array(Box::new(PropertyType::Int)),
            ));
        }
        schema.add_property(PropertyDef::new("digest", PropertyType::Binary));
        schema.add_property(PropertyDef::new("query", PropertyType::Binary));
        schema.add_property(PropertyDef::new("working", PropertyType::Binary));
        schema.add_property(PropertyDef::new("payload", PropertyType::Binary));
        for name in ["choices", "audiences", "editors", "columns"] {
            schema.add_property(PropertyDef::new(
                name,
                PropertyType::Array(Box::new(PropertyType::String)),
            ));
        }
        schema.add_property(PropertyDef::new(
            "kinds",
            PropertyType::Array(Box::new(PropertyType::Int)),
        ));
        let mut intent = Intent::Ensure;
        if let Some(current) = db.label_schemas()?.into_iter().find(|s| s.name == "OmRow")
            && current.properties != schema.properties
        {
            let root = db.execute_cypher_shared(
                "MATCH (n:OmRow {key:'installation'}) RETURN n.`schema` AS version",
                None,
                None,
                None,
                None,
            )?;
            let previous_version = root.rows.first().and_then(|row| row.get("version"));
            if !matches!(
                root.rows.first().and_then(|row| row.get("version")),
                Some(Value::Int(2..=6))
            ) || current.mode != schema.mode
                || current.placement != schema.placement
                || current.temporal != schema.temporal
                || current
                    .properties
                    .iter()
                    .any(|(key, value)| schema.properties.get(key) != Some(value))
                || schema
                    .properties
                    .keys()
                    .filter(|key| !current.properties.contains_key(*key))
                    .any(|key| {
                        if key == "count" || key == "bytes" {
                            return false;
                        }
                        if previous_version == Some(&Value::Int(6)) {
                            return true;
                        }
                        if previous_version == Some(&Value::Int(3)) {
                            return key != "payload" && key != "virtualization_role";
                        }
                        if matches!(previous_version, Some(Value::Int(4 | 5))) {
                            return key != "virtualization_role";
                        }
                        ![
                            "address",
                            "meaning",
                            "zone",
                            "binding",
                            "ip_order",
                            "prefix_length",
                            "working",
                            "payload",
                            "virtualization_role",
                            "count",
                            "bytes",
                        ]
                        .contains(&key.as_str())
                    })
            {
                return Err(Error::SchemaVersion);
            }
            intent = Intent::Change {
                expected_revision: current.schema_revision,
            };
        }
        let publication = db.publish_schema(SchemaPackage {
            labels: vec![LabelDeclaration { schema, intent }],
            edge_types: vec![
                EdgeTypeDeclaration {
                    schema: EdgeTypeSchema::new("OmRelationFrom"),
                    intent: Intent::Ensure,
                },
                EdgeTypeDeclaration {
                    schema: EdgeTypeSchema::new("OmRelationTo"),
                    intent: Intent::Ensure,
                },
            ],
            constraints: vec![PackageConstraint {
                name: Some("omloom_key".into()),
                label: "OmRow".into(),
                properties: vec!["key".into()],
                kind: ConstraintKind::Unique,
                scope: None,
                on_duplicate_rename: None,
            }],
        })?;
        if !db
            .await_schema(&publication, Duration::from_secs(20))?
            .all_ready()
        {
            return Err(Error::SchemaVersion);
        }
        for (name, property) in [
            ("omloom_parent", "parent"),
            ("omloom_class", "class"),
            ("omloom_layer", "layer"),
            // Reverse membership lookup and shared-address expansion for visible interface hints.
            ("omloom_asset", "asset"),
            ("omloom_group", "group"),
        ] {
            match db.execute_cypher(&format!("CREATE INDEX {name} ON :OmRow({property})")) {
                Ok(_) => {}
                Err(DatabaseError::CatalogObjectExists {
                    object: coordinode_embed::CatalogObject::Index,
                    name: existing,
                }) if existing == name => {}
                Err(error) => return Err(error.into()),
            }
        }
        let store = Self { db, policy };
        if store.read(ROOT)?.is_none() {
            let root = store.new_root()?;
            store.db.execute_cypher_shared(
                "CREATE (n:OmRow {key:$key}) SET n += $p",
                params([("key", text(ROOT)), ("p", Value::Map(root))]),
                None,
                None,
                None,
            )?;
        }
        let root = store.read(ROOT)?.ok_or(Error::Corrupt)?;
        let version = number(&root, "schema")?;
        if !(2..=SCHEMA_VERSION).contains(&version) {
            return Err(Error::SchemaVersion);
        }
        if string(&root, "policy")? != store.policy_identity() {
            return Err(Error::PolicyChanged);
        }
        if version == 2 {
            workspace::upgrade_network_order(&store)?;
        }
        if version < 7 {
            workspace::network_facts::upgrade(&store)?;
        }
        if version < SCHEMA_VERSION {
            // One-direction admission of scoped address facts. Older readers
            // must refuse the new kind; unbound old previews expire on acceptance.
            let mut tx = store.begin()?;
            if number(&tx.root, "schema")? < SCHEMA_VERSION {
                tx.root.insert("schema".into(), int(SCHEMA_VERSION)?);
                tx.changed = true;
            }
            tx.commit()?;
        }
        // CoordiNode stores a member-local recovery guard before an index's
        // first use. Prepare our fixed indexes before exposing a ready store.
        let class_probe = format!(
            "MATCH (n:OmRow {{class:{}}}) RETURN n.key AS key LIMIT 1",
            Class::Inventory as usize
        );
        for query in [
            "MATCH (n:OmRow {parent:'assets'}) RETURN n.key AS key LIMIT 1",
            &class_probe,
            "MATCH (n:OmRow {layer:'intent'}) RETURN n.key AS key LIMIT 1",
        ] {
            store
                .db
                .execute_cypher_shared(query, None, None, None, None)?;
        }
        Ok(store)
    }
    fn policy_identity(&self) -> String {
        let mut encoded = format!(
            "v1/{}/{}/{}/{}/{}/{}",
            self.policy.total_bytes,
            self.policy.detail_age_ms,
            self.policy.batch_rows,
            self.policy.review_age_ms,
            self.policy.review_per_source.rows,
            self.policy.review_per_source.bytes
        );
        use std::fmt::Write;
        for budget in &self.policy.classes {
            write!(&mut encoded, "/{}/{}", budget.rows, budget.bytes)
                .expect("formatting into String");
        }
        encoded
    }
    fn new_root(&self) -> Result<Properties> {
        Ok(props([
            ("key", text(ROOT)),
            ("schema", int(SCHEMA_VERSION)?),
            ("revision", int(1)?),
            ("policy", text(self.policy_identity())),
            ("restored", Value::Bool(false)),
            ("counts", Value::Array(vec![Value::Int(0); CLASSES])),
            ("sizes", Value::Array(vec![Value::Int(0); CLASSES])),
        ]))
    }
    fn read(&self, key: &str) -> Result<Option<Properties>> {
        let result = self.db.execute_cypher_shared(
            "MATCH (n:OmRow {key:$key}) RETURN properties(n) AS p",
            params([("key", text(key))]),
            None,
            None,
            None,
        )?;
        Ok(unpack(result.rows)?.pop())
    }
    fn begin(&self) -> Result<Tx<'_>> {
        // Read the version before beginning the snapshot. Any intervening commit
        // then fails this guard; a newer version cannot bless an older snapshot.
        let rows = self
            .db
            .execute_cypher_shared(
                "MATCH (n:OmRow {key:'installation'}) RETURN id(n) AS id",
                None,
                None,
                None,
                None,
            )?
            .rows;
        let raw = rows
            .first()
            .and_then(|r| r.get("id"))
            .and_then(Value::as_int)
            .ok_or(Error::Corrupt)?;
        let id = NodeId::from_raw(u64::try_from(raw).map_err(|_| Error::Corrupt)?);
        let version = self.db.node_version(id)?;
        let tx = self.db.begin_transaction()?;
        let result = (|| {
            self.db.expect_node_version(tx, id, version)?;
            let mut rows = unpack(self.db.execute_in_transaction(
                tx,
                "MATCH (n:OmRow {key:'installation'}) RETURN properties(n) AS p",
                None,
            )?)?;
            let root = rows.pop().ok_or(Error::Corrupt)?;
            Ok(Tx {
                store: self,
                id: Some(tx),
                root,
                changed: false,
            })
        })();
        if result.is_err() {
            self.db.rollback_transaction(tx)?;
        }
        result
    }
    pub fn usage(&self) -> Result<Usage> {
        usage(&self.read(ROOT)?.ok_or(Error::Corrupt)?)
    }
    pub fn query_plan(&self) -> Result<String> {
        Ok(self
            .db
            .explain_cypher("MATCH (n:OmRow {key:'asset/example'}) RETURN n")?)
    }
    /// Opt-in inspection for consumer tests; no default-build instrumentation.
    #[cfg(feature = "diagnostics")]
    pub fn diagnostic_database(&self) -> &Database {
        &self.db
    }
}

fn usage(root: &Properties) -> Result<Usage> {
    fn array(root: &Properties, key: &str) -> Result<[u64; CLASSES]> {
        let Some(Value::Array(values)) = root.get(key) else {
            return Err(Error::Corrupt);
        };
        let mut output = [0; CLASSES];
        if values.len() != CLASSES {
            return Err(Error::Corrupt);
        }
        for (out, value) in output.iter_mut().zip(values) {
            *out = value
                .as_int()
                .and_then(|n| u64::try_from(n).ok())
                .ok_or(Error::Corrupt)?;
        }
        Ok(output)
    }
    Ok(Usage {
        rows: array(root, "counts")?,
        bytes: array(root, "sizes")?,
        revision: number(root, "revision")?,
    })
}

struct Tx<'a> {
    store: &'a Store,
    id: Option<TransactionId>,
    root: Properties,
    changed: bool,
}
impl Tx<'_> {
    fn query(&self, q: &str, p: Option<HashMap<String, Value>>) -> Result<Vec<Properties>> {
        unpack(
            self.store
                .db
                .execute_in_transaction(self.id.ok_or(Error::Corrupt)?, q, p)?,
        )
    }
    fn get(&self, key: &str) -> Result<Option<Properties>> {
        Ok(self
            .query(
                "MATCH (n:OmRow {key:$key}) RETURN properties(n) AS p",
                params([("key", text(key))]),
            )?
            .pop())
    }
    fn children(&self, parent: &str) -> Result<Vec<Properties>> {
        let rows = self.query(
            "MATCH (n:OmRow) WHERE n.parent=$parent RETURN properties(n) AS p LIMIT 2049",
            params([("parent", text(parent))]),
        )?;
        if rows.len() > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        Ok(rows)
    }
    fn counters(&mut self, old: Option<&Properties>, new: Option<&Properties>) -> Result<()> {
        let mut usage = usage(&self.root)?;
        for (row, adding) in [(old, false), (new, true)] {
            if let Some(row) = row {
                let class = usize::try_from(number(row, "class")?).map_err(|_| Error::Corrupt)?;
                if class >= CLASSES {
                    return Err(Error::Corrupt);
                }
                let size = number(row, "size")?;
                if adding {
                    usage.rows[class] = usage.rows[class].checked_add(1).ok_or(Error::Limit)?;
                    usage.bytes[class] =
                        usage.bytes[class].checked_add(size).ok_or(Error::Limit)?;
                } else {
                    usage.rows[class] = usage.rows[class].checked_sub(1).ok_or(Error::Corrupt)?;
                    usage.bytes[class] =
                        usage.bytes[class].checked_sub(size).ok_or(Error::Corrupt)?;
                }
            }
        }
        self.set_usage(usage)
    }
    fn set_usage(&mut self, usage: Usage) -> Result<()> {
        let mut total = 0u64;
        for (index, budget) in self.store.policy.classes.iter().enumerate() {
            if usage.rows[index] > budget.rows || usage.bytes[index] > budget.bytes {
                return Err(Error::Limit);
            }
            total = total.checked_add(usage.bytes[index]).ok_or(Error::Limit)?;
        }
        if total > self.store.policy.total_bytes {
            return Err(Error::Limit);
        }
        self.root.insert(
            "counts".into(),
            Value::Array(usage.rows.into_iter().map(int).collect::<Result<_>>()?),
        );
        self.root.insert(
            "sizes".into(),
            Value::Array(usage.bytes.into_iter().map(int).collect::<Result<_>>()?),
        );
        Ok(())
    }
    fn settlement_reservation(&mut self, old: crate::Budget, new: crate::Budget) -> Result<()> {
        let mut usage = usage(&self.root)?;
        let class = Class::Detail as usize;
        usage.rows[class] = usage.rows[class]
            .checked_sub(old.rows)
            .ok_or(Error::Corrupt)?
            .checked_add(new.rows)
            .ok_or(Error::Limit)?;
        usage.bytes[class] = usage.bytes[class]
            .checked_sub(old.bytes)
            .ok_or(Error::Corrupt)?
            .checked_add(new.bytes)
            .ok_or(Error::Limit)?;
        self.set_usage(usage)?;
        self.changed = true;
        Ok(())
    }
    fn put(&mut self, key: &str, parent: &str, class: Class, data: Properties) -> Result<bool> {
        let old = self.get(key)?;
        self.put_against(key, parent, class, data, old.as_ref())
    }
    /// A preimage already read at this transaction's snapshot can be reused:
    /// the installation CAS guard protects it through the final commit.
    fn put_against(
        &mut self,
        key: &str,
        parent: &str,
        class: Class,
        data: Properties,
        old: Option<&Properties>,
    ) -> Result<bool> {
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        self.put_projection(key, parent, class, data, old)
    }
    /// Format migration can rebuild derived columns while the restored
    /// installation remains fenced against domain mutations and dispatch.
    fn put_projection(
        &mut self,
        key: &str,
        parent: &str,
        class: Class,
        data: Properties,
        old: Option<&Properties>,
    ) -> Result<bool> {
        let data = charged_row(key, parent, class, data)?;
        if old == Some(&data) {
            return Ok(false);
        }
        self.counters(old, Some(&data))?;
        let query = if old.is_some() {
            "MATCH (n:OmRow {key:$key}) SET n = $p"
        } else {
            "CREATE (n:OmRow {key:$key}) SET n = $p"
        };
        self.store.db.execute_in_transaction(
            self.id.ok_or(Error::Corrupt)?,
            query,
            params([("key", text(key)), ("p", Value::Map(data))]),
        )?;
        self.changed = true;
        Ok(true)
    }
    fn delete(&mut self, key: &str) -> Result<()> {
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        self.delete_projection(key)
    }
    fn delete_projection(&mut self, key: &str) -> Result<()> {
        if let Some(old) = self.get(key)? {
            self.counters(Some(&old), None)?;
            self.store.db.execute_in_transaction(
                self.id.ok_or(Error::Corrupt)?,
                "MATCH (n:OmRow {key:$key}) DELETE n",
                params([("key", text(key))]),
            )?;
            self.changed = true;
        }
        Ok(())
    }
    fn commit(mut self) -> Result<u64> {
        let mut revision = number(&self.root, "revision")?;
        if self.changed {
            revision = revision.checked_add(1).ok_or(Error::Limit)?;
            self.root.insert("revision".into(), int(revision)?);
            self.store.db.execute_in_transaction(
                self.id.ok_or(Error::Corrupt)?,
                "MATCH (n:OmRow {key:'installation'}) SET n = $p",
                params([("p", Value::Map(core::mem::take(&mut self.root)))]),
            )?;
            self.store
                .db
                .commit_transaction(self.id.take().ok_or(Error::Corrupt)?)?;
        } else {
            self.store
                .db
                .rollback_transaction(self.id.take().ok_or(Error::Corrupt)?)?;
        }
        Ok(revision)
    }
    fn asset(&self, target: AssetRef) -> Result<Properties> {
        let p = self.get(&asset_key(target))?.ok_or(Error::Missing)?;
        if crate::disk::target(&p)? != target {
            return Err(Error::Conflict);
        }
        Ok(p)
    }
    fn audit(
        &mut self,
        owner: &str,
        actor: &str,
        action: &str,
        revision: u64,
        now: u64,
    ) -> Result<()> {
        bounded(actor)?;
        self.put(
            &format!("audit/{owner}/{revision}"),
            &format!("detail/{owner}"),
            Class::Detail,
            audit_data(actor, action, revision, now)?,
        )?;
        Ok(())
    }
}
impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            // Explicit paths report errors. Drop is only exception cleanup and
            // must not panic while an original database error is being returned.
            if let Err(error) = self.store.db.rollback_transaction(id) {
                eprintln!("transaction cleanup failed: {error}");
            }
        }
    }
}
