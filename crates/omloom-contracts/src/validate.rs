use crate::{Error, ids, wire::*};
use std::collections::{BTreeMap, BTreeSet};

fn need<T>(value: &Option<T>) -> Result<&T, Error> {
    value.as_ref().ok_or(Error::Missing)
}
fn positive(value: u64) -> Result<(), Error> {
    if value == 0 {
        Err(Error::Value)
    } else {
        Ok(())
    }
}
fn text(value: &str, max: usize, empty: bool) -> Result<(), Error> {
    if value.len() > max || (!empty && value.is_empty()) || value.chars().any(char::is_control) {
        Err(Error::Value)
    } else {
        Ok(())
    }
}
fn token(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        Err(Error::Value)
    } else {
        Ok(())
    }
}
fn target(value: &AssetRef) -> Result<(), Error> {
    ids::AssetRef::try_from(value).map(|_| ())
}
fn kind(value: i32) -> Result<(), Error> {
    if value == AssetKind::Unspecified as i32 || AssetKind::try_from(value).is_err() {
        return Err(Error::Enumeration);
    }
    Ok(())
}
fn source(value: &SourceIdentity) -> Result<(), Error> {
    ids::check(&value.connection_id)?;
    source_object(need(&value.object)?)
}
fn source_object(value: &SourceObject) -> Result<(), Error> {
    token(&value.object_kind)?;
    text(&value.object_id, 256, false)?;
    if let Some(evidence) = &value.incarnation_evidence {
        text(evidence, 256, false)?;
    }
    Ok(())
}
fn evidence(value: &Evidence, scan_id: Option<&str>) -> Result<(), Error> {
    ids::check(&value.source_id)?;
    if value.source_revision.is_none() && scan_id.is_none() {
        return Err(Error::Missing);
    }
    if let Some(rev) = &value.source_revision {
        text(rev, 256, false)?;
    }
    positive(value.observed_at_ms)?;
    // JS BigInt is used for u64, including instants; no lossy Number conversion.
    if value.valid_until_ms < value.observed_at_ms {
        return Err(Error::Value);
    }
    unique_fields(&value.fields)
}
fn field(value: &Field) -> Result<(), Error> {
    token(&value.name)?;
    match need(&value.value)? {
        field::Value::Text(s) => text(s, 4096, true),
        field::Value::Equipment(v) => crate::equipment::validate_field(&value.name, v),
        field::Value::Clear(v) if *v != NullValue::NullValue as i32 => Err(Error::Enumeration),
        _ => Ok(()),
    }
}
fn unique_fields<'a>(values: impl IntoIterator<Item = &'a Field>) -> Result<(), Error> {
    // Decode reservation includes the fixed 256-entry validation envelope.
    let mut names = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        if index == 256 {
            return Err(Error::Limit);
        }
        field(value)?;
        if !names.insert(value.name.as_str()) {
            return Err(Error::Duplicate);
        }
    }
    Ok(())
}
fn observation(value: &Observation) -> Result<(), Error> {
    if let Some(scan) = &value.scan_id {
        ids::check(scan)?;
    }
    evidence(need(&value.evidence)?, value.scan_id.as_deref())
}
fn current_observations(values: &[Observation]) -> Result<(), Error> {
    if values.len() > 256 {
        return Err(Error::Limit);
    }
    // The preflight reservation covers these indexes before any allocation.
    // Index current source/field claims, never independent historical samples.
    let mut sources = BTreeMap::new();
    let mut fields = BTreeSet::new();
    let mut windows = BTreeSet::new();
    for value in values {
        observation(value)?;
        let ev = need(&value.evidence)?;
        if ev.fields.is_empty() {
            return Err(Error::Missing);
        }
        let source = ev.source_id.as_str();
        if !sources.contains_key(source) && sources.len() == 32 {
            return Err(Error::Limit);
        }
        let count = sources.entry(source).or_insert(0);
        *count += ev.fields.len();
        if *count > 256 {
            return Err(Error::Limit);
        }
        for field in &ev.fields {
            if !fields.insert((source, field.name.as_str())) {
                return Err(Error::Duplicate);
            }
        }
        if !windows.insert((
            source,
            ev.source_revision.as_deref(),
            value.scan_id.as_deref(),
            ev.observed_at_ms,
            ev.valid_until_ms,
        )) {
            // Identically attributed fields belong in one compact group.
            return Err(Error::Duplicate);
        }
    }
    Ok(())
}
fn candidate(value: &Candidate) -> Result<(), Error> {
    ids::check(&value.candidate_id)?;
    source(need(&value.source)?)?;
    kind(value.kind)?;
    observation(need(&value.observation)?)
}
fn asset(value: &Asset) -> Result<(), Error> {
    target(need(&value.target)?)?;
    kind(value.kind)?;
    if !matches!(
        Adoption::try_from(value.adoption),
        Ok(Adoption::Adopted | Adoption::Archived)
    ) {
        return Err(Error::Enumeration);
    }
    positive(value.revision)?;
    if value.intended.len() > 256 || value.sources.len() > 32 {
        return Err(Error::Limit);
    }
    for intent in &value.intended {
        ids::check(&intent.authority_id)?;
        need(&intent.field)?;
    }
    unique_fields(value.intended.iter().filter_map(|v| v.field.as_ref()))?;
    current_observations(&value.imported)?;
    current_observations(&value.observed)?;
    let mut sources = BTreeSet::new();
    for entry in &value.sources {
        source(entry)?;
        let object = need(&entry.object)?;
        need(&object.incarnation_evidence)?;
        if !sources.insert((&entry.connection_id, &object.object_kind, &object.object_id)) {
            return Err(Error::Duplicate);
        }
    }
    Ok(())
}
fn scan(value: &ScanBatch) -> Result<(), Error> {
    ids::check(&value.scan_id)?;
    ids::check(&value.connection_id)?;
    text(&value.cursor, 256, false)?;
    if let Some(cursor) = &value.prior_cursor {
        text(cursor, 256, false)?;
    }
    if let Some(rev) = &value.snapshot_revision {
        text(rev, 256, false)?;
    }
    match ScanCompleteness::try_from(value.completeness) {
        Ok(ScanCompleteness::SnapshotComplete) => {
            need(&value.snapshot_revision)?;
        }
        Ok(ScanCompleteness::Partial) => (),
        Ok(ScanCompleteness::AccessDenied) => {
            if !value.candidates.is_empty() {
                return Err(Error::Value);
            }
        }
        _ => return Err(Error::Enumeration),
    }
    if value.candidates.len() > 128 {
        return Err(Error::Limit);
    }
    let mut identities = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    for entry in &value.candidates {
        ids::check(&entry.candidate_id)?;
        kind(entry.kind)?;
        let src = need(&entry.source)?;
        source_object(src)?;
        evidence(need(&entry.evidence)?, Some(&value.scan_id))?;
        if !identities.insert((&src.object_kind, &src.object_id))
            || !candidates.insert(&entry.candidate_id)
        {
            return Err(Error::Duplicate);
        }
    }
    Ok(())
}
fn relation(value: &Relation) -> Result<(), Error> {
    ids::check(&value.relation_id)?;
    target(need(&value.from)?)?;
    target(need(&value.to)?)?;
    if value.kind == RelationKind::Unspecified as i32 || RelationKind::try_from(value.kind).is_err()
    {
        return Err(Error::Enumeration);
    }
    positive(value.revision)?;
    ids::check(&value.authority_id)
}
fn scope(value: &ConnectionScope) -> Result<(), Error> {
    target(need(&value.source)?)?;
    target(need(&value.destination)?)?;
    if !matches!(
        Direction::try_from(value.direction),
        Ok(Direction::Forward | Direction::Bidirectional)
    ) {
        return Err(Error::Enumeration);
    }
    if let Some(id) = &value.vrf_id {
        ids::check(id)?;
    }
    if let Some(vlan) = value.vlan
        && !(1..=4094).contains(&vlan)
    {
        return Err(Error::Value);
    }
    token(&value.service)
}
fn provider(value: &ProviderConnection) -> Result<(), Error> {
    ids::check(&value.connection_id)?;
    ids::check(&value.trust_id)?;
    if let Some(id) = &value.credential_binding_id {
        ids::check(id)?;
    }
    token(&value.adapter)?;
    text(&value.endpoint, 2048, false)?;
    let url = url::Url::parse(&value.endpoint).map_err(|_| Error::Value)?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.as_str() != value.endpoint
    {
        return Err(Error::Value);
    }
    Ok(())
}
fn requirements(value: &Requirements) -> Result<(), Error> {
    if value.version != 1 {
        return Err(Error::Value);
    }
    let count = value.nodes.len();
    if count == 0 || count > 256 {
        return Err(Error::Limit);
    }
    if value.root as usize >= count {
        return Err(Error::Reference);
    }
    // Fixed stack storage: no attacker-controlled recursion or graph allocation.
    let mut remaining = [0u16; 256];
    let mut removed = [false; 256];
    let mut edges = 0;
    for (index, node) in value.nodes.iter().enumerate() {
        match RequirementKind::try_from(node.kind) {
            Ok(RequirementKind::Fact) => {
                if !node.children.is_empty() || node.complete {
                    return Err(Error::Value);
                }
                target(need(&node.resource)?)?;
                token(&node.fact)?;
            }
            Ok(RequirementKind::All | RequirementKind::Any) => {
                if node.children.is_empty() || node.resource.is_some() || !node.fact.is_empty() {
                    return Err(Error::Value);
                }
            }
            _ => return Err(Error::Enumeration),
        }
        if node.children.len() > 64 {
            return Err(Error::Limit);
        }
        edges += node.children.len();
        if edges > 1024 {
            return Err(Error::Limit);
        }
        let mut seen = [false; 256];
        for &child in &node.children {
            if child as usize >= count {
                return Err(Error::Reference);
            }
            if seen[child as usize] {
                return Err(Error::Duplicate);
            }
            seen[child as usize] = true;
        }
        remaining[index] = node.children.len() as u16;
    }
    // Bounded O(V*(V+E)), max 256*(256+1024). Evaluation uses a later reverse index.
    for _ in 0..count {
        let Some(index) = (0..count).find(|&i| !removed[i] && remaining[i] == 0) else {
            return Err(Error::Cycle);
        };
        removed[index] = true;
        for (parent, node) in value.nodes.iter().enumerate() {
            for &child in &node.children {
                if child as usize == index {
                    remaining[parent] -= 1;
                }
            }
        }
    }
    // Every admitted node must contribute to this root. Mark on enqueue so a
    // shared descendant is visited once and the fixed frontier cannot overflow.
    let mut seen = [false; 256];
    let mut pending = [0usize; 256];
    pending[0] = value.root as usize;
    seen[value.root as usize] = true;
    let mut len = 1;
    let mut visited = 1;
    while len != 0 {
        len -= 1;
        let node = pending[len];
        for &child in &value.nodes[node].children {
            let child = child as usize;
            if !seen[child] {
                seen[child] = true;
                debug_assert!(len < count);
                pending[len] = child;
                len += 1;
                visited += 1;
            }
        }
    }
    if visited != count {
        return Err(Error::Reference);
    }
    Ok(())
}
fn operation(value: &Operation) -> Result<(), Error> {
    ids::check(&value.operation_id)?;
    target(need(&value.target)?)?;
    positive(value.inventory_revision)?;
    positive(value.revision)?;
    if value.plan_digest.len() != 32 {
        return Err(Error::Value);
    }
    if let Some(task) = &value.provider_task_id {
        text(task, 256, false)?;
    }
    let state = OperationState::try_from(value.state).map_err(|_| Error::Enumeration)?;
    let outcome = EffectOutcome::try_from(value.outcome).map_err(|_| Error::Enumeration)?;
    use EffectOutcome::*;
    use OperationState::*;
    let valid = match state {
        Draft | Validated | Queued | Blocked => outcome == NotSent,
        Running | CancelRequested => matches!(outcome, Unknown | Confirmed | ConfirmedNoEffect),
        Verifying | Succeeded => outcome == Confirmed,
        Reconciling | NeedsAttention => matches!(outcome, Unknown | Confirmed | ConfirmedNoEffect),
        Failed => matches!(outcome, NotSent | ConfirmedNoEffect | Confirmed),
        Cancelled => matches!(outcome, NotSent | ConfirmedNoEffect),
        OperationState::Unspecified => false,
    };
    if !valid || (outcome == NotSent && value.provider_task_id.is_some()) {
        return Err(Error::Outcome);
    }
    Ok(())
}

pub(crate) fn record(value: &Record) -> Result<(), Error> {
    match need(&value.body)? {
        record::Body::Asset(v) => asset(v),
        record::Body::Candidate(v) => candidate(v),
        record::Body::Scan(v) => scan(v),
        record::Body::Relation(v) => relation(v),
        record::Body::InfrastructureConnection(v) => {
            ids::check(&v.connection_id)?;
            positive(v.revision)?;
            scope(need(&v.scope)?)?;
            requirements(need(&v.requirements)?)
        }
        record::Body::ProviderConnection(v) => provider(v),
        record::Body::Operation(v) => operation(v),
        record::Body::Requirements(v) => requirements(v),
    }
}
