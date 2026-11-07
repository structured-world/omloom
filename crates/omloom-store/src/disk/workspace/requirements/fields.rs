use super::*;

/// One evaluation owns this bounded read snapshot; it is never persisted or shared across requests.
#[derive(Default)]
pub(super) struct Inputs {
    pub assets: BTreeMap<String, Properties>,
    pub accepted: BTreeMap<String, Properties>,
    pub observed: BTreeMap<(String, String), Vec<Properties>>,
    pub freshness: BTreeMap<String, Properties>,
}

impl Inputs {
    pub fn load(
        tx: &Tx<'_>,
        context: &Context,
        request: &api::EvaluateRequirements,
        selected: &[core::Decision],
    ) -> Result<Self> {
        let mut conditions = Vec::with_capacity(request.nodes.len());
        let mut keys = BTreeSet::new();
        for (node, decision) in request.nodes.iter().zip(selected) {
            if decision.reason == core::Reason::NotSelected {
                continue;
            }
            if let Some(api::requirement_node::Expression::Field(condition)) = &node.expression {
                validate_field_condition(condition)?;
                keys.insert(asset_key(parse_target(condition.target.as_ref())?));
                conditions.push(condition);
            }
        }
        let mut input = Self::default();
        if keys.is_empty() {
            return Ok(input);
        }
        for row in tx.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {key:n.key,asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption,revision:n.revision} AS p LIMIT 65",
            params([("keys", Value::Array(keys.iter().map(text).collect()))]),
        )? {
            input.assets.insert(string(&row, "key")?, row);
        }
        let mut accepted = BTreeSet::new();
        let mut observed = BTreeSet::new();
        for condition in conditions {
            let target = parse_target(condition.target.as_ref())?;
            let parent = asset_key(target);
            let Some(row) = input.assets.get(&parent) else {
                continue;
            };
            let kind = number(row, "kind")? as i32;
            let field = &condition.expected.as_ref().ok_or(Error::Invalid)?.name;
            // Do not fetch a protected field payload, even if another requested field is allowed.
            if crate::disk::target(row)? != target
                || !context.readable(target, kind)
                || !context.field(target, kind, field, false)
                || number(row, "adoption")? != wire::Adoption::Adopted as u64
            {
                continue;
            }
            match api::RequirementLayer::try_from(condition.layer).map_err(|_| Error::Invalid)? {
                api::RequirementLayer::Accepted => {
                    accepted.insert(format!("{parent}/intent/{field}"));
                }
                api::RequirementLayer::Observed => {
                    observed.insert((parent, field.clone()));
                }
                _ => return Err(Error::Invalid),
            }
        }
        if !accepted.is_empty() {
            for row in tx.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 65",
                params([("keys", Value::Array(accepted.iter().map(text).collect()))]),
            )? {
                input.accepted.insert(string(&row, "key")?, row);
            }
        }
        if observed.is_empty() {
            return Ok(input);
        }
        let lookups = observed
            .iter()
            .map(|(parent, field)| {
                Value::Map(props([("parent", text(parent)), ("field", text(field))]))
            })
            .collect();
        let rows = tx.query("UNWIND $lookups AS lookup MATCH (n:OmRow {parent:lookup.parent}) WHERE n.layer='observed' AND n.field=lookup.field RETURN properties(n) AS p LIMIT 2049",
            params([("lookups", Value::Array(lookups))]))?;
        if rows.len() > 2048 {
            return Err(Error::Limit);
        }
        let mut claims = Vec::with_capacity(rows.len());
        let mut parents = BTreeSet::new();
        for row in rows {
            let parent = string(&row, "parent")?;
            let field = string(&row, "field")?;
            if !observed.contains(&(parent.clone(), field.clone())) {
                return Err(Error::Corrupt);
            }
            parents.insert(parent.clone());
            claims.push(text(string(&row, "key")?));
            let values = input.observed.entry((parent, field)).or_default();
            if values.len() == 32 {
                return Err(Error::Limit);
            }
            values.push(row);
        }
        if !claims.is_empty() {
            let windows = tx.query("UNWIND $parents AS parent MATCH (n:OmRow {parent:parent}) WHERE n.layer='freshness' AND n.claim IN $keys RETURN properties(n) AS p LIMIT 2049",
                params([("parents", Value::Array(parents.iter().map(text).collect())), ("keys", Value::Array(claims))]))?;
            if windows.len() > 2048 {
                return Err(Error::Limit);
            }
            for row in windows {
                input.freshness.insert(string(&row, "claim")?, row);
            }
        }
        Ok(input)
    }
}
