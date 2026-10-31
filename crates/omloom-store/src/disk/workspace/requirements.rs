//! Current selected facts enter the common evaluator through this restricted query.
use super::*;
use crate::requirements::{self as core, knowledge as k};

impl k::Alternatives for wire::EquipmentAlternatives {
    fn candidates(&self) -> usize {
        self.candidates.len()
    }
    fn candidate(&self, index: usize) -> k::Atom<'_> {
        let value = &self.candidates[index];
        value
            .reference
            .as_ref()
            .map_or(k::Atom::Text(&value.value), reference)
    }
    fn exclusions(&self) -> usize {
        self.excluded.len()
    }
    fn exclusion(&self, index: usize) -> k::Atom<'_> {
        let value = &self.excluded[index];
        value
            .reference
            .as_ref()
            .map_or(k::Atom::Text(&value.value), reference)
    }
    fn exhaustive(&self) -> bool {
        self.exhaustive
    }
}
fn reference(value: &wire::EquipmentReference) -> k::Atom<'_> {
    k::Atom::Reference(k::Reference {
        issuer: &value.issuer,
        key: &value.key,
        basis: &value.basis,
    })
}
fn claim(field: &wire::Field) -> k::Claim<'_, wire::EquipmentAlternatives> {
    match field.value.as_ref() {
        Some(field::Value::Text(value)) => k::Claim::Known(k::Atom::Text(value)),
        Some(field::Value::Integer(value)) => k::Claim::Known(k::Atom::Integer(*value)),
        Some(field::Value::Flag(value)) => k::Claim::Known(k::Atom::Flag(*value)),
        Some(field::Value::Equipment(value)) => match value.assertion.as_ref() {
            Some(wire::equipment_claim::Assertion::Model(value)) => {
                k::Claim::Known(reference(value))
            }
            Some(wire::equipment_claim::Assertion::Alternatives(value)) => {
                k::Claim::Alternatives(value)
            }
            Some(wire::equipment_claim::Assertion::Quantity(value)) => {
                k::Claim::Quantity(k::Quantity {
                    lower: value.lower,
                    upper: value.upper,
                    unit: &value.unit,
                    scale: value.scale,
                })
            }
            None => k::Claim::Missing,
        },
        _ => k::Claim::Missing,
    }
}
fn compare(
    actual: &wire::Field,
    expected: &wire::Field,
    comparison: i32,
) -> Result<core::Decision> {
    use api::RequirementComparison as C;
    let comparison = C::try_from(comparison).map_err(|_| Error::Invalid)?;
    let expected = claim(expected);
    let mut atoms = [k::Atom::Text(""); 16];
    let predicate = match (comparison, expected) {
        (C::Equal, k::Claim::Known(atom)) => k::Predicate::Is(atom),
        (C::Equal, k::Claim::Alternatives(values))
            if values.exhaustive
                && !values.candidates.is_empty()
                && values.candidates.len() <= 16
                && values.excluded.is_empty()
                && values.calibration_basis.is_none()
                && values
                    .candidates
                    .iter()
                    .all(|c| c.probability_ppm.is_none() && c.score_ppm.is_none()) =>
        {
            for (index, atom) in atoms[..values.candidates.len()].iter_mut().enumerate() {
                *atom = k::Alternatives::candidate(values, index);
            }
            k::Predicate::OneOf(&atoms[..values.candidates.len()])
        }
        (C::Minimum | C::Maximum, k::Claim::Known(k::Atom::Integer(value))) => {
            if comparison == C::Minimum {
                k::Predicate::Minimum {
                    value,
                    unit: "count",
                    scale: 0,
                }
            } else {
                k::Predicate::Maximum {
                    value,
                    unit: "count",
                    scale: 0,
                }
            }
        }
        (C::Minimum | C::Maximum, k::Claim::Quantity(q))
            if q.lower.is_some() && q.lower == q.upper && q.scale <= 6 =>
        {
            let value = q.lower.ok_or(Error::Invalid)?;
            if comparison == C::Minimum {
                k::Predicate::Minimum {
                    value,
                    unit: q.unit,
                    scale: q.scale,
                }
            } else {
                k::Predicate::Maximum {
                    value,
                    unit: q.unit,
                    scale: q.scale,
                }
            }
        }
        _ => return Err(Error::Invalid),
    };
    Ok(k::compare(claim(actual), predicate))
}
fn wire_decision(decision: core::Decision) -> (i32, i32) {
    use crate::requirements::{Reason as C, State};
    use api::{RequirementReason as R, RequirementState as S};
    let state = match decision.state {
        State::Satisfied => S::Satisfied,
        State::Failed => S::Failed,
        State::Unknown => S::Unknown,
    };
    let reason = match decision.reason {
        C::Match => R::Match,
        C::Mismatch => R::Mismatch,
        C::Missing => R::Missing,
        C::Stale => R::Stale,
        C::Conflict => R::Conflict,
        C::Restricted => R::Restricted,
        C::Unsupported => R::Unsupported,
        C::Unconfigured => R::Unconfigured,
        C::AuthorityUnavailable => R::AuthorityUnavailable,
        C::OwnerActionRequired => R::OwnerActionRequired,
        C::Incomplete => R::Incomplete,
        C::ScopeMismatch => R::ScopeMismatch,
        C::VantageMismatch => R::VantageMismatch,
        C::Unqualified => R::Unqualified,
        C::WrongEvidence => R::WrongEvidence,
        C::Cycle => R::Cycle,
        C::NotSelected => R::Unspecified,
    };
    (state as i32, reason as i32)
}
fn subject(target: &wire::AssetRef) -> core::Subject<'_> {
    core::Subject {
        namespace: "inventory",
        resource: &target.asset_id,
        incarnation: &target.incarnation,
        network: core::NetworkScope::NotApplicable,
    }
}

fn reference_identity(value: &wire::EquipmentReference) -> (&str, &str, &str) {
    (&value.issuer, &value.key, &value.basis)
}
fn group_mode(group: &api::RequirementGroup) -> Result<core::Group> {
    match api::RequirementGroupKind::try_from(group.kind).map_err(|_| Error::Invalid)? {
        api::RequirementGroupKind::All if group.minimum == 0 => Ok(core::Group::All),
        api::RequirementGroupKind::Any if group.minimum == 0 => Ok(core::Group::Any),
        api::RequirementGroupKind::AtLeast
            if group.minimum != 0 && (group.minimum as usize) <= group.members.len() =>
        {
            Ok(core::Group::AtLeast(group.minimum as usize))
        }
        _ => Err(Error::Invalid),
    }
}
fn semantic_keys(nodes: &[api::RequirementNode]) -> Result<Vec<String>> {
    let mut digests = [None; core::MAX_NODES];
    for (index, node) in nodes.iter().enumerate() {
        if let Some(api::requirement_node::Expression::Field(condition)) = &node.expression {
            let mut condition = condition.clone();
            if let Some(wire::Field {
                value: Some(field::Value::Equipment(claim)),
                ..
            }) = &mut condition.expected
            {
                // Narrative and candidate ordering are not part of a predicate's identity.
                claim.basis.clear();
                if let Some(wire::equipment_claim::Assertion::Alternatives(set)) =
                    &mut claim.assertion
                {
                    set.domain.clear();
                    for candidate in &mut set.candidates {
                        if candidate.reference.is_some() {
                            candidate.value.clear();
                        }
                    }
                    set.candidates.sort_unstable_by(|a, b| {
                        a.value.cmp(&b.value).then_with(|| {
                            a.reference
                                .as_ref()
                                .map(reference_identity)
                                .cmp(&b.reference.as_ref().map(reference_identity))
                        })
                    });
                }
                if let Some(wire::equipment_claim::Assertion::Quantity(q)) = &mut claim.assertion {
                    if q.scale > 6 {
                        return Err(Error::Invalid);
                    }
                    while q.scale != 0
                        && q.lower.is_some_and(|n| n % 10 == 0)
                        && q.upper.is_some_and(|n| n % 10 == 0)
                    {
                        q.lower = q.lower.map(|n| n / 10);
                        q.upper = q.upper.map(|n| n / 10);
                        q.scale -= 1;
                    }
                }
            }
            digests[index] = Some(<[u8; 32]>::from(Sha256::digest(condition.encode_to_vec())));
        }
    }
    // At most 64 bounded passes, without recursion or keys dependent on node numbering.
    for _ in 0..nodes.len() {
        let mut changed = false;
        for (index, node) in nodes.iter().enumerate() {
            if digests[index].is_some() {
                continue;
            }
            let Some(api::requirement_node::Expression::Group(group)) = &node.expression else {
                return Err(Error::Invalid);
            };
            let mut children = [[0u8; 32]; core::MAX_NODES];
            let mut ready = true;
            for (position, &child) in group.members.iter().enumerate() {
                let Some(child) = digests
                    .get(child as usize)
                    .filter(|_| (child as usize) < nodes.len())
                else {
                    return Err(Error::Invalid);
                };
                if let Some(child) = child {
                    children[position] = *child;
                } else {
                    ready = false;
                    break;
                }
            }
            if !ready {
                continue;
            }
            children[..group.members.len()].sort_unstable();
            let mut hash = Sha256::new();
            hash.update(b"group");
            hash.update(group.kind.to_le_bytes());
            hash.update(group.minimum.to_le_bytes());
            for child in &children[..group.members.len()] {
                hash.update(child);
            }
            digests[index] = Some(hash.finalize().into());
            changed = true;
        }
        if !changed {
            break;
        }
    }
    Ok(nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            // Invalid cyclic graphs remain explainable but never acquire a successful result.
            let digest = digests[i].unwrap_or_else(|| Sha256::digest(node.encode_to_vec()).into());
            format!("{:x}", sha2::digest::Output::<Sha256>::from(digest))
        })
        .collect())
}

impl Store {
    pub(super) fn workspace_requirements(
        &self,
        context: &Context,
        request: api::EvaluateRequirements,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if request.nodes.is_empty()
            || request.nodes.len() > core::MAX_NODES
            || request.root as usize >= request.nodes.len()
        {
            return Err(Error::Invalid);
        }
        let mut total_edges = 0;
        for node in &request.nodes {
            match &node.expression {
                Some(api::requirement_node::Expression::Field(condition)) => {
                    if condition.encoded_len() > 8192 {
                        return Err(Error::Limit);
                    }
                }
                Some(api::requirement_node::Expression::Group(group)) => {
                    if group.members.len() > core::MAX_NODES
                        || group.members.len() > core::MAX_EDGES - total_edges
                    {
                        return Err(Error::Limit);
                    }
                    total_edges += group.members.len();
                }
                None => return Err(Error::Invalid),
            }
        }
        let semantic_keys = semantic_keys(&request.nodes)?;
        let anchor = parse_target(request.anchor.as_ref())?;
        let tx = self.begin()?;
        tx.selection(context, working, false)?
            .contains(anchor, &tx)?;
        tx.visible_asset(context, anchor)?;
        // Reserve the finite containers once; fields are selected by their indexed parent.
        let mut decisions = Vec::with_capacity(request.nodes.len());
        let mut responses = Vec::with_capacity(request.nodes.len());
        let mut members = Vec::with_capacity(request.nodes.len());
        let mut edges = 0;
        for ((index, node), semantic_key) in request.nodes.iter().enumerate().zip(semantic_keys) {
            let response = api::RequirementResult {
                node: index as u32,
                semantic_key,
                ..Default::default()
            };
            let decision = match node.expression.as_ref().ok_or(Error::Invalid)? {
                api::requirement_node::Expression::Field(_) => {
                    core::Decision::unknown(core::Reason::Missing)
                }
                api::requirement_node::Expression::Group(group) => {
                    if group.members.len() > core::MAX_EDGES - edges {
                        return Err(Error::Limit);
                    }
                    edges += group.members.len();
                    core::Decision::unknown(core::Reason::NotSelected)
                }
            };
            decisions.push(decision);
            responses.push(response);
            members.push(match &node.expression {
                Some(api::requirement_node::Expression::Group(group)) => group
                    .members
                    .iter()
                    .map(|&i| i as usize)
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            });
        }
        let mut nodes = Vec::with_capacity(request.nodes.len());
        let keys: Vec<_> = responses
            .iter_mut()
            .map(|r| std::mem::take(&mut r.semantic_key))
            .collect();
        for (index, node) in request.nodes.iter().enumerate() {
            let (subject, expression) = match node.expression.as_ref().ok_or(Error::Invalid)? {
                api::requirement_node::Expression::Field(condition) => (
                    subject(condition.target.as_ref().ok_or(Error::Invalid)?),
                    core::Expression::Fact(decisions[index]),
                ),
                api::requirement_node::Expression::Group(_) => {
                    (
                        core::Subject {
                            namespace: "requirement",
                            resource: &keys[index],
                            incarnation: "",
                            network: core::NetworkScope::NotApplicable,
                        },
                        core::Expression::Group {
                            // The first pass discovers reachability without choosing a method.
                            mode: core::Group::All,
                            members: &members[index],
                            membership: core::Membership {
                                complete: true,
                                consensus: false,
                                qualified_profile: None,
                                revision: None,
                            },
                        },
                    )
                }
            };
            nodes.push(core::Node {
                subject,
                role: core::Role::Requirement,
                // A client-built logical group is not a qualified independent voter.
                member_role: if matches!(
                    node.expression,
                    Some(api::requirement_node::Expression::Field(_))
                ) {
                    core::MemberRole::Eligible
                } else {
                    core::MemberRole::Unknown
                },
                expression,
            });
        }
        let mut results = [core::Decision::unknown(core::Reason::NotSelected); core::MAX_NODES];
        let map_error = |e| match e {
            core::Error::Invalid => Error::Invalid,
            core::Error::Limit => Error::Limit,
        };
        let shape =
            core::evaluate(&nodes, request.root as usize, &mut results).map_err(map_error)?;
        if shape.reason != core::Reason::Cycle {
            // Validate all selected group semantics before loading any field evidence.
            for (index, node) in nodes.iter_mut().enumerate() {
                if results[index].reason == core::Reason::NotSelected {
                    continue;
                }
                if let Some(api::requirement_node::Expression::Group(group)) =
                    &request.nodes[index].expression
                    && let core::Expression::Group { mode, .. } = &mut node.expression
                {
                    *mode = group_mode(group)?;
                }
            }
            for (index, node) in nodes.iter_mut().enumerate() {
                if results[index].reason == core::Reason::NotSelected {
                    continue;
                }
                if let Some(api::requirement_node::Expression::Field(condition)) =
                    request.nodes[index].expression.as_ref()
                {
                    node.expression = core::Expression::Fact(tx.requirement_field(
                        context,
                        condition,
                        &mut responses[index],
                        now,
                    )?);
                }
            }
        }
        let overall =
            core::evaluate(&nodes, request.root as usize, &mut results).map_err(map_error)?;
        drop(nodes);
        let mut output = Vec::with_capacity(request.nodes.len());
        for ((index, mut response), semantic_key) in responses.into_iter().enumerate().zip(keys) {
            if results[index].reason == core::Reason::NotSelected {
                continue;
            }
            response.semantic_key = semantic_key;
            if results[index].state == core::State::Satisfied {
                response.resolutions.clear();
            }
            (response.state, response.reason) = wire_decision(results[index]);
            output.push(response);
        }
        let (state, reason) = wire_decision(overall);
        let evaluation = api::RequirementEvaluation {
            revision: number(&tx.root, "revision")?,
            evaluated_at_ms: now,
            state,
            reason,
            results: output,
        };
        tx.commit()?;
        Ok(reply(api::reply::Result::RequirementEvaluation(evaluation)))
    }
}

impl Tx<'_> {
    fn requirement_field(
        &self,
        context: &Context,
        condition: &api::FieldRequirement,
        response: &mut api::RequirementResult,
        now: u64,
    ) -> Result<core::Decision> {
        let target = parse_target(condition.target.as_ref())?;
        let expected = condition.expected.as_ref().ok_or(Error::Invalid)?;
        if expected.name.is_empty()
            || expected.name.len() > 64
            || !expected
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || expected.encoded_len() > 8192
        {
            return Err(Error::Invalid);
        }
        // Validate the desired predicate before any source lookup, including missing facts.
        if let Some(field::Value::Equipment(value)) = &expected.value {
            omloom_contracts::equipment::validate_field(&expected.name, value)
                .map_err(|_| Error::Invalid)?;
        }
        compare(expected, expected, condition.comparison)?;
        let row = match self.asset(target) {
            Ok(row) => row,
            // A stale incarnation must not reveal that a protected asset ID exists.
            Err(Error::Missing | Error::Conflict) => {
                return Ok(core::Decision::unknown(core::Reason::Restricted));
            }
            Err(error) => return Err(error),
        };
        let kind = number(&row, "kind")? as i32;
        let asset_revision = number(&row, "revision")?;
        if !context.readable(target, kind)
            || !context.field(target, kind, &expected.name, false)
            || number(&row, "adoption")? != wire::Adoption::Adopted as u64
        {
            return Ok(core::Decision::unknown(core::Reason::Restricted));
        }
        response.target = condition.target.clone();
        response.field = Some(expected.name.clone());
        if context.field(target, kind, &expected.name, true) {
            response
                .resolutions
                .push(api::RequirementResolution::RecordFact as i32);
        }
        let layer =
            match api::RequirementLayer::try_from(condition.layer).map_err(|_| Error::Invalid)? {
                api::RequirementLayer::Accepted => "intent",
                api::RequirementLayer::Observed => "observed",
                _ => return Err(Error::Invalid),
            };
        if kind == wire::AssetKind::ProviderAccount as i32 && expected.name == "delegation" {
            if layer != "intent" {
                return Ok(core::Decision::unknown(core::Reason::Unconfigured));
            }
            let Some(metadata) = self.get(&format!("{}/account", asset_key(target)))? else {
                return Ok(core::Decision::unknown(core::Reason::Missing));
            };
            let delegation =
                api::AccountDelegation::try_from(number(&metadata, "delegation")? as i32)
                    .map_err(|_| Error::Corrupt)?;
            if delegation == api::AccountDelegation::Unspecified {
                return Ok(core::Decision::unknown(core::Reason::Missing));
            }
            let actual = wire::Field {
                name: "delegation".into(),
                value: Some(field::Value::Text(delegation.as_str_name().into())),
            };
            let decision = compare(&actual, expected, condition.comparison)?;
            response.evidence.push(api::RequirementEvidence {
                source: target.asset.to_string(),
                revision: Some(asset_revision.to_string()),
                ..Default::default()
            });
            if delegation == api::AccountDelegation::OwnerOnly
                && decision.state == core::State::Failed
            {
                if context.field(target, kind, "account.request", true) {
                    response
                        .resolutions
                        .push(api::RequirementResolution::OwnerAction as i32);
                }
                return Ok(core::Decision {
                    state: core::State::Failed,
                    reason: core::Reason::OwnerActionRequired,
                });
            }
            return Ok(decision);
        }
        let rows = if layer == "intent" {
            self.get(&format!("{}/intent/{}", asset_key(target), expected.name))?
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.layer='observed' AND n.field=$field RETURN properties(n) AS p LIMIT 33", params([("parent", text(asset_key(target))), ("field", text(&expected.name))]))?
        };
        if rows.len() > 32 {
            return Err(Error::Limit);
        }
        let freshness = if layer == "observed" && !rows.is_empty() {
            let keys = rows
                .iter()
                .map(|row| row.get("key").cloned().ok_or(Error::Corrupt))
                .collect::<Result<Vec<_>>>()?;
            self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.layer='freshness' AND n.claim IN $keys RETURN properties(n) AS p LIMIT 33", params([("parent", text(asset_key(target))), ("keys", Value::Array(keys))]))?
        } else {
            Vec::new()
        };
        if freshness.len() > 32 {
            return Err(Error::Limit);
        }
        let freshness: BTreeMap<_, _> = freshness
            .iter()
            .map(|row| {
                Ok((
                    row.get("claim")
                        .and_then(Value::as_str)
                        .ok_or(Error::Corrupt)?,
                    row,
                ))
            })
            .collect::<Result<_>>()?;
        let mut result = core::Decision::unknown(core::Reason::Missing);
        let mut known = None;
        let mut current_fields = Vec::with_capacity(rows.len());
        for row in &rows {
            let (source, revision, at, until, scan_id) = if layer == "intent" {
                (
                    string(row, "authority")?,
                    Some(asset_revision.to_string()),
                    None,
                    None,
                    None,
                )
            } else {
                let key = row
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or(Error::Corrupt)?;
                let fresh = freshness.get(key).ok_or(Error::Corrupt)?;
                (
                    string(row, "source")?,
                    fresh
                        .get("source_revision")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    Some(number(fresh, "observed")?),
                    Some(number(fresh, "valid")?),
                    fresh.get("scan").and_then(Value::as_str).map(str::to_owned),
                )
            };
            response.evidence.push(api::RequirementEvidence {
                source,
                revision,
                observed_at_ms: at,
                valid_until_ms: until,
                scan_id,
            });
            let required = core::Requirement {
                subject: subject(condition.target.as_ref().ok_or(Error::Invalid)?),
                semantic: &expected.name,
                vantage: "recorded-source-claim",
                phase: core::Phase::Current,
                kind: if layer == "intent" {
                    core::EvidenceKind::Configuration
                } else {
                    core::EvidenceKind::Observation
                },
                interval: None,
            };
            // A recorded source claim is attributed evidence, never an independent probe.
            let validity = core::fact(
                required,
                core::Evidence {
                    requirement: required,
                    origin: if layer == "intent" {
                        core::Origin::Accepted
                    } else {
                        core::Origin::PeerAssertion
                    },
                    observed_at: at,
                    valid_until: until,
                    coverage: None,
                    qualified: true,
                    decision: core::Decision::PASS,
                },
                now,
            );
            if validity.state != core::State::Satisfied {
                if known.is_none() {
                    result = validity;
                }
                continue;
            }
            let actual = inventory::read_field(row)?;
            let current = compare(&actual, expected, condition.comparison)?;
            current_fields.push(actual);
            if current.state != core::State::Unknown {
                if known.is_some_and(|old| old != current.state) {
                    return Ok(core::Decision::unknown(core::Reason::Conflict));
                }
                known = Some(current.state);
                result = current;
            } else if known.is_none() {
                result = current;
            }
            if current.reason == core::Reason::Conflict {
                return Ok(current);
            }
        }
        let claims: Vec<_> = current_fields.iter().map(claim).collect();
        let consistency = k::consistent(&claims);
        if consistency.state != core::State::Satisfied {
            return Ok(consistency);
        }
        if result.state != core::State::Satisfied {
            response
                .resolutions
                .push(api::RequirementResolution::ReviewSource as i32);
        }
        Ok(result)
    }
}
