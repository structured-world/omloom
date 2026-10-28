//! Application projection and mutations on the existing transactional namespace.
//! Context is supplied by a trusted host, never decoded from a caller's request.
use super::*;
use omloom_contracts::{ids::*, workspace as api};
use prost::Message;
use std::collections::BTreeSet;

mod accounts;
mod addresses;
mod assets;
mod current_facts;
mod definitions;
mod exchange;
mod interface_facts;
mod network;
pub(super) mod network_facts;
mod projection;
mod responsibility;
mod views;
mod working;

pub const MAX_PAGE: usize = 100;
/// Maximum protobuf reply bytes, shared with the host transport ceiling.
pub const MAX_REPLY_BYTES: usize = 512 * 1024;
pub const MAX_BULK: usize = 128;
pub const MAX_QUERY: usize = 256;
pub const MAX_IMPORT: usize = 128;
pub const MAX_CSV: usize = 256 * 1024;
pub const MAX_FIELDS: usize = 32;
pub const MAX_DEFINITIONS: usize = 128;
const EXPLICIT_CLEAR: i32 = wire::NullValue::NullValue as i32;

pub(super) fn upgrade_asset_names(store: &Store) -> Result<()> {
    let mut after = String::new();
    loop {
        let mut tx = store.begin()?;
        let rows = tx.query("MATCH (n:OmRow {parent:'assets'}) WHERE n.key>$after RETURN properties(n) AS p ORDER BY n.key LIMIT 128", params([("after", text(&after))]))?;
        if rows.is_empty() {
            tx.commit()?;
            return Ok(());
        }
        for old in &rows {
            let key = string(old, "key")?;
            if let Some(field) = tx.get(&format!("{key}/intent/name"))?
                && let Some(field::Value::Text(name)) = inventory::read_field(&field)?.value
            {
                let mut data = old.clone();
                operations::strip_envelope(&mut data);
                data.insert("name_sort".into(), text(name.to_lowercase()));
                data.insert("name".into(), text(name));
                // Rebuild only derived columns, preserving domain revisions and restore fences.
                tx.put_projection(&key, "assets", Class::Inventory, data, Some(old))?;
            }
            after = key;
        }
        tx.commit()?;
    }
}

pub(super) fn upgrade_network_order(store: &Store) -> Result<()> {
    loop {
        let mut tx = store.begin()?;
        let rows = tx.query("MATCH (n:OmRow {parent:'networks'}) WHERE n.cidr IS NOT NULL AND n.ip_order IS NULL RETURN properties(n) AS p LIMIT 128",None)?;
        if rows.is_empty() {
            tx.commit()?;
            return Ok(());
        }
        for old in rows {
            let block: ipnet::IpNet = string(&old, "cidr")?.parse().map_err(|_| Error::Corrupt)?;
            let mut data = old.clone();
            super::operations::strip_envelope(&mut data);
            data.insert(
                "ip_order".into(),
                text(network::address_order(block.addr())),
            );
            data.insert("prefix_len".into(), int(u64::from(block.prefix_len()))?);
            tx.put_projection(
                &string(&old, "key")?,
                "networks",
                Class::Inventory,
                data,
                Some(&old),
            )?;
        }
        tx.commit()?;
    }
}

#[derive(Debug, Clone)]
pub enum Scope {
    All,
    Kinds(Vec<wire::AssetKind>),
    Targets(Vec<AssetRef>),
}
#[derive(Debug, Clone)]
pub enum Fields {
    All,
    Selected(Vec<String>),
}
impl Fields {
    fn contains(&self, field: &str) -> bool {
        match self {
            Self::All => true,
            Self::Selected(fields) => fields.iter().any(|f| f == field),
        }
    }
}
#[derive(Debug, Clone)]
pub struct Grant {
    pub scope: Scope,
    pub read: Fields,
    pub write: Fields,
    pub writable: bool,
}
#[derive(Debug, Clone)]
pub struct Context {
    pub actor: String,
    pub fixture: String,
    pub grants: Vec<Grant>,
    pub teams: Vec<String>,
    pub recipients: Vec<String>,
    pub schema_admin: bool,
    pub authority_revision: u64,
}
impl Context {
    fn matches(grant: &Grant, target: AssetRef, kind: i32) -> bool {
        match &grant.scope {
            Scope::All => true,
            Scope::Kinds(kinds) => kinds.iter().any(|k| *k as i32 == kind),
            Scope::Targets(targets) => targets.contains(&target),
        }
    }
    fn readable(&self, target: AssetRef, kind: i32) -> bool {
        self.grants.iter().any(|g| Self::matches(g, target, kind))
    }
    fn field(&self, target: AssetRef, kind: i32, field: &str, write: bool) -> bool {
        if kind == wire::AssetKind::AddressReference as i32
            && field == "name"
            && !self.field(target, kind, "address", write)
        {
            return false;
        }
        self.grants.iter().any(|g| {
            Self::matches(g, target, kind)
                && if write {
                    g.writable && g.write.contains(field)
                } else {
                    g.read.contains(field)
                }
        })
    }
    fn admin(&self) -> Result<()> {
        if self.schema_admin {
            Ok(())
        } else {
            Err(Error::Denied)
        }
    }
}

fn target_wire(target: AssetRef) -> wire::AssetRef {
    wire::AssetRef {
        asset_id: target.asset.to_string(),
        incarnation: target.incarnation.to_string(),
    }
}
fn parse_target(value: Option<&wire::AssetRef>) -> Result<AssetRef> {
    AssetRef::try_from(value.ok_or(Error::Invalid)?).map_err(|_| Error::Invalid)
}
fn new_target() -> AssetRef {
    AssetRef {
        asset: AssetId::generate(),
        incarnation: IncarnationId::generate(),
    }
}
fn reply(result: api::reply::Result) -> api::Reply {
    api::Reply {
        result: Some(result),
    }
}
fn mutation(revision: u64, targets: Vec<wire::AssetRef>) -> api::Reply {
    reply(api::reply::Result::Mutation(api::Mutation {
        revision,
        targets,
        affected: 0,
    }))
}
fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(text).collect())
}
fn read_strings(p: &Properties, key: &str) -> Result<Vec<String>> {
    match p.get(key) {
        Some(Value::Array(values)) => values
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or(Error::Corrupt))
            .collect(),
        _ => Err(Error::Corrupt),
    }
}
fn flag(p: &Properties, key: &str) -> Result<bool> {
    p.get(key).and_then(Value::as_bool).ok_or(Error::Corrupt)
}
fn field_value<'a>(fields: &'a [wire::Field], name: &str) -> Option<&'a field::Value> {
    fields
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| f.value.as_ref())
}
pub(super) fn hot_fields(asset: &wire::Asset, data: &mut Properties) {
    for intended in &asset.intended {
        if let Some(field) = &intended.field {
            hot_field(field, data);
        }
    }
}
// Projection accepts the stored provider contract; workspace input limits apply before writes.
fn hot_field(field: &wire::Field, data: &mut Properties) {
    if let Some(field::Value::Text(value)) = &field.value {
        match field.name.as_str() {
            "name" => {
                data.insert("name".into(), text(value));
                data.insert("name_sort".into(), text(value.to_lowercase()));
            }
            "virtualization_role" => {
                data.insert("virtualization_role".into(), text(value));
            }
            _ => {}
        }
    }
}
fn binary<'a>(p: &'a Properties, key: &str) -> Result<&'a [u8]> {
    match p.get(key) {
        Some(Value::Binary(bytes)) => Ok(bytes),
        _ => Err(Error::Corrupt),
    }
}

impl Store {
    /// Executes an application request against real disk state. The host has
    /// already bounded the frame, acquired work capacity and selected context.
    pub fn workspace(
        &self,
        context: &Context,
        request: api::Request,
        now: u64,
    ) -> Result<api::Reply> {
        bounded(&context.actor)?;
        if context.grants.len() > 32 || context.teams.len() > 32 {
            return Err(Error::Invalid);
        }
        use api::request::Command as C;
        let working = request.working;
        match request.command.ok_or(Error::Invalid)? {
            C::ObserveProxmox(_) => Err(Error::Invalid),
            C::Identity(_) => Ok(reply(api::reply::Result::Identity(api::Identity {
                name: context.actor.clone(),
                fixture: context.fixture.clone(),
                teams: context.teams.clone(),
                writable: context.grants.iter().any(|g| g.writable),
                schema_admin: context.schema_admin,
            }))),
            C::Query(query) => self
                .workspace_query(context, query)
                .map(|p| reply(api::reply::Result::Page(p))),
            C::Read(read) => self
                .workspace_read(context, read, working.as_ref().ok_or(Error::Invalid)?)
                .map(|d| reply(api::reply::Result::Detail(Box::new(d)))),
            C::Write(write) => {
                self.workspace_write(context, write, working.as_ref().ok_or(Error::Invalid)?, now)
            }
            C::Create(create) => self.workspace_create(
                context,
                create,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::Bulk(bulk) => {
                self.workspace_bulk(context, bulk, working.as_ref().ok_or(Error::Invalid)?, now)
            }
            C::Relation(relation) => self.workspace_relation(
                context,
                relation,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::Definitions(_) => self
                .workspace_definitions(context)
                .map(|items| reply(api::reply::Result::Definitions(api::Definitions { items }))),
            C::Definition(definition) => self.workspace_definition(context, definition, now),
            C::Migration(migration) => self.workspace_migration(context, migration, now),
            C::Views(_) => self
                .workspace_views(context)
                .map(|items| reply(api::reply::Result::Views(api::Views { items }))),
            C::SaveView(view) => self.workspace_save_view(context, view, now),
            C::Import(import) => self.workspace_import(
                context,
                import,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::Export(export) => self.workspace_export(context, export),
            C::Network(network) => self.workspace_network(
                context,
                network,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::Member(member) => self.workspace_member(
                context,
                member,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::Prepare(prepare) => {
                self.workspace_prepare(context, working.as_ref().ok_or(Error::Invalid)?, prepare)
            }
            C::Address(address) => self.workspace_address(
                context,
                address,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::HostNetwork(evidence) => self.workspace_host_network(
                context,
                evidence,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::InterfaceAddresses(value) => self.workspace_interface_addresses(
                context,
                value,
                working.as_ref().ok_or(Error::Invalid)?,
            ),
            C::Responsibility(value) => self.workspace_responsibility(
                context,
                value,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::ExternalAccount(value) => {
                self.workspace_account(context, value, working.as_ref().ok_or(Error::Invalid)?, now)
            }
            C::OwnerRequest(value) => self.workspace_owner_request(
                context,
                value,
                working.as_ref().ok_or(Error::Invalid)?,
                now,
            ),
            C::ResponsibilityTransfer(value) => self
                .responsibility_transfer(
                    context,
                    value,
                    working.as_ref().ok_or(Error::Invalid)?,
                    now,
                )
                .map(|state| reply(api::reply::Result::ResponsibilityTransfer(state))),
        }
    }

    fn workspace_rows(&self, parent: &str, limit: usize) -> Result<Vec<Properties>> {
        let tx = self.begin()?;
        let rows = tx.query(
            "MATCH (n:OmRow) WHERE n.parent=$parent RETURN properties(n) AS p LIMIT $limit",
            params([
                ("parent", text(parent)),
                ("limit", int((limit + 1) as u64)?),
            ]),
        )?;
        if rows.len() > limit {
            return Err(Error::Limit);
        }
        tx.commit()?;
        Ok(rows)
    }
}

impl Tx<'_> {
    fn raw(
        &self,
        query: &str,
        values: HashMap<String, Value>,
    ) -> Result<Vec<BTreeMap<String, Value>>> {
        self.execute(query, Some(values))
    }
    fn visible_asset(&self, context: &Context, target: AssetRef) -> Result<Properties> {
        let row = self.asset(target)?;
        if number(&row, "adoption")? != wire::Adoption::Adopted as u64 {
            return Err(Error::Missing);
        }
        if !context.readable(target, number(&row, "kind")? as i32) {
            return Err(Error::Denied);
        }
        Ok(row)
    }
    fn require_changes(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
        fields: &[wire::Field],
    ) -> Result<()> {
        if fields.is_empty() || fields.len() > MAX_FIELDS {
            return Err(Error::Invalid);
        }
        let mut seen = BTreeSet::new();
        for field in fields {
            if !seen.insert(&field.name) || !context.field(target, kind, &field.name, true) {
                return Err(Error::Denied);
            }
            self.validate_field(kind, field)?;
        }
        Ok(())
    }
}
