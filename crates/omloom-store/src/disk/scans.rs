use super::*;
use omloom_contracts::ids::{ConnectionId, ScanId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanState {
    pub scan: ScanId,
    pub epoch: u64,
    pub cursor: Option<String>,
    pub completeness: wire::ScanCompleteness,
    pub snapshot_revision: Option<String>,
    pub review_rows: u64,
    pub review_bytes: u64,
}
fn head_key(connection: &str) -> String {
    format!("scan/{connection}")
}
fn candidate_key(connection: &str, kind: &str, id: &str) -> String {
    format!(
        "candidate/{connection}/{}/{}",
        hex(kind.as_bytes()),
        hex(id.as_bytes())
    )
}

// Canonical sets are hashed through borrowed, bounded stack slots. Reordering
// a page cannot change its intent, and hashing never copies the whole frame.
fn scan_digest(batch: &wire::ScanBatch) -> Result<[u8; 32]> {
    if batch.candidates.len() > 128 {
        return Err(Error::Limit);
    }
    let mut hash = Sha256::new();
    hash.update(b"omloom/scan-batch/v1");
    hash_part(&mut hash, batch.scan_id.as_bytes());
    hash_part(&mut hash, batch.connection_id.as_bytes());
    hash_optional(&mut hash, batch.prior_cursor.as_deref());
    hash_part(&mut hash, batch.cursor.as_bytes());
    hash.update(batch.completeness.to_be_bytes());
    hash_optional(&mut hash, batch.snapshot_revision.as_deref());
    hash.update((batch.candidates.len() as u64).to_be_bytes());
    let mut candidates = [None; 128];
    for (slot, candidate) in candidates.iter_mut().zip(&batch.candidates) {
        if candidate.source.is_none() || candidate.evidence.is_none() {
            return Err(Error::Invalid);
        }
        *slot = Some(candidate);
    }
    let ordered = &mut candidates[..batch.candidates.len()];
    ordered.sort_unstable_by(|left, right| {
        let left = left
            .expect("initialized candidate slot")
            .source
            .as_ref()
            .expect("checked source");
        let right = right
            .expect("initialized candidate slot")
            .source
            .as_ref()
            .expect("checked source");
        (&left.object_kind, &left.object_id).cmp(&(&right.object_kind, &right.object_id))
    });
    let mut fields = [None; 256];
    for candidate in ordered {
        let candidate = candidate.expect("initialized candidate slot");
        let source = candidate.source.as_ref().ok_or(Error::Invalid)?;
        let evidence = candidate.evidence.as_ref().ok_or(Error::Invalid)?;
        for value in [
            &candidate.candidate_id,
            &source.object_kind,
            &source.object_id,
        ] {
            hash_part(&mut hash, value.as_bytes());
        }
        hash_optional(&mut hash, source.incarnation_evidence.as_deref());
        hash.update(candidate.kind.to_be_bytes());
        hash_part(&mut hash, evidence.source_id.as_bytes());
        hash_optional(&mut hash, evidence.source_revision.as_deref());
        hash.update(evidence.observed_at_ms.to_be_bytes());
        hash.update(evidence.valid_until_ms.to_be_bytes());
        if evidence.fields.len() > 256 {
            return Err(Error::Limit);
        }
        hash.update((evidence.fields.len() as u64).to_be_bytes());
        for (slot, field) in fields.iter_mut().zip(&evidence.fields) {
            *slot = Some(field);
        }
        let ordered_fields = &mut fields[..evidence.fields.len()];
        ordered_fields.sort_unstable_by(|left, right| {
            left.expect("initialized field slot")
                .name
                .cmp(&right.expect("initialized field slot").name)
        });
        for field in ordered_fields {
            hash_field(&mut hash, field.expect("initialized field slot"))?;
        }
    }
    Ok(hash.finalize().into())
}
fn scan_state(p: &Properties) -> Result<ScanState> {
    Ok(ScanState {
        scan: string(p, "scan")?.parse().map_err(|_| Error::Corrupt)?,
        epoch: number(p, "epoch")?,
        cursor: p.get("cursor").and_then(Value::as_str).map(str::to_owned),
        completeness: wire::ScanCompleteness::try_from(number(p, "completeness")? as i32)
            .map_err(|_| Error::Corrupt)?,
        snapshot_revision: p.get("snapshot").and_then(Value::as_str).map(str::to_owned),
        review_rows: number(p, "review_rows")?,
        review_bytes: number(p, "review_bytes")?,
    })
}

impl Store {
    /// Allocate the current scan identity under a durable per-source epoch.
    /// A lost response is recovered with scan_state, not by starting another scan.
    pub fn start_scan(
        &self,
        connection: ConnectionId,
        expected_epoch: Option<u64>,
    ) -> Result<ScanState> {
        let key = head_key(&connection.to_string());
        let mut tx = self.begin()?;
        let old = tx.get(&key)?;
        if old.as_ref().map(|p| number(p, "epoch")).transpose()? != expected_epoch {
            return Err(Error::Conflict);
        }
        let epoch = expected_epoch
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let scan = ScanId::generate();
        let mut p = props([
            ("scan", text(scan.to_string())),
            ("epoch", int(epoch)?),
            ("completeness", int(wire::ScanCompleteness::Partial as u64)?),
            (
                "review_rows",
                int(old
                    .as_ref()
                    .map(|p| number(p, "review_rows"))
                    .transpose()?
                    .unwrap_or(0))?,
            ),
            (
                "review_bytes",
                int(old
                    .as_ref()
                    .map(|p| number(p, "review_bytes"))
                    .transpose()?
                    .unwrap_or(0))?,
            ),
        ]);
        let state = scan_state(&p)?;
        p.insert("connection".into(), text(connection.to_string()));
        tx.put_against(&key, "scans", Class::Checkpoint, p, old.as_ref())?;
        tx.commit()?;
        Ok(state)
    }
    pub fn scan_state(&self, connection: ConnectionId) -> Result<ScanState> {
        scan_state(
            &self
                .read(&head_key(&connection.to_string()))?
                .ok_or(Error::Missing)?,
        )
    }

    /// Candidates and source checkpoint commit together. Complete/denied scans
    /// never delete accepted inventory, and retired scan IDs cannot advance a head.
    pub fn ingest_scan(&self, record: &Record, now: u64) -> Result<()> {
        let Some(record::Body::Scan(batch)) = &record.wire().body else {
            return Err(Error::Invalid);
        };
        let digest = scan_digest(batch)?;
        let key = head_key(&batch.connection_id);
        let mut tx = self.begin()?;
        let mut head = tx.get(&key)?.ok_or(Error::Missing)?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        if head.get("scan").and_then(Value::as_str) != Some(batch.scan_id.as_str()) {
            return Err(Error::Conflict);
        }
        if head.get("cursor").and_then(Value::as_str) == Some(batch.cursor.as_str()) {
            if stored_digest(&head)? != digest {
                return Err(Error::Conflict);
            }
            tx.commit()?;
            return Ok(());
        }
        if head.get("cursor").and_then(Value::as_str) != batch.prior_cursor.as_deref() {
            return Err(Error::Conflict);
        }
        let before = usage(&tx.root)?;
        let expires = now
            .checked_add(self.policy.review_age_ms)
            .ok_or(Error::Invalid)?;
        int(expires)?;
        for candidate in &batch.candidates {
            let object = candidate.source.as_ref().ok_or(Error::Invalid)?;
            let evidence = candidate.evidence.as_ref().ok_or(Error::Invalid)?;
            let candidate_key =
                candidate_key(&batch.connection_id, &object.object_kind, &object.object_id);
            let previous = tx.children(&candidate_key)?;
            let by_field: BTreeMap<&str, &Properties> = previous
                .iter()
                .map(|p| {
                    Ok((
                        p.get("field")
                            .and_then(Value::as_str)
                            .ok_or(Error::Corrupt)?,
                        p,
                    ))
                })
                .collect::<Result<_>>()?;
            let keep: std::collections::BTreeSet<&str> =
                evidence.fields.iter().map(|f| f.name.as_str()).collect();
            for p in &previous {
                let name = p
                    .get("field")
                    .and_then(Value::as_str)
                    .ok_or(Error::Corrupt)?;
                if !keep.contains(name) {
                    tx.delete(&string(p, "key")?)?;
                }
            }
            let expires = expires.min(evidence.valid_until_ms);
            let p = props([
                ("candidate", text(&candidate.candidate_id)),
                ("connection", text(&batch.connection_id)),
                ("object_kind", text(&object.object_kind)),
                ("object_id", text(&object.object_id)),
                (
                    "lifetime",
                    object
                        .incarnation_evidence
                        .as_ref()
                        .map_or(Value::Null, text),
                ),
                ("kind", Value::Int(candidate.kind.into())),
                ("scan", text(&batch.scan_id)),
                ("source", text(&evidence.source_id)),
                (
                    "source_revision",
                    evidence.source_revision.as_ref().map_or(Value::Null, text),
                ),
                ("observed", int(evidence.observed_at_ms)?),
                ("valid", int(evidence.valid_until_ms)?),
                ("expires", int(expires)?),
            ]);
            tx.put(
                &candidate_key,
                &format!("review/{}", batch.connection_id),
                Class::Review,
                p,
            )?;
            for field in &evidence.fields {
                let old = by_field.get(field.name.as_str()).copied();
                if old.is_some_and(|p| inventory::field_matches(field, p)) {
                    continue;
                }
                let mut p = inventory::field_props(field)?;
                p.extend(props([
                    ("connection", text(&batch.connection_id)),
                    ("expires", int(expires)?),
                ]));
                tx.put_against(
                    &format!("{candidate_key}/{}", field.name),
                    &candidate_key,
                    Class::Review,
                    p,
                    old,
                )?;
            }
        }
        update_review_scope(&mut head, &before, &usage(&tx.root)?, &self.policy)?;
        head.insert("cursor".into(), text(&batch.cursor));
        head.insert("digest".into(), digest_value(digest));
        head.insert("completeness".into(), Value::Int(batch.completeness.into()));
        head.insert(
            "snapshot".into(),
            batch.snapshot_revision.as_ref().map_or(Value::Null, text),
        );
        operations::strip_envelope(&mut head);
        tx.put(&key, "scans", Class::Checkpoint, head)?;
        tx.commit()?;
        Ok(())
    }

    /// Retrieve one current, expiring review object. This does not adopt it.
    pub fn candidate(
        &self,
        connection: ConnectionId,
        kind: &str,
        id: &str,
        now: u64,
    ) -> Result<wire::Candidate> {
        if !token(kind) {
            return Err(Error::Invalid);
        }
        bounded(id)?;
        let key = candidate_key(&connection.to_string(), kind, id);
        let tx = self.begin()?;
        let p = tx.get(&key)?.ok_or(Error::Missing)?;
        let head = tx
            .get(&head_key(&connection.to_string()))?
            .ok_or(Error::Corrupt)?;
        if p.get("scan") != head.get("scan") {
            return Err(Error::Expired);
        }
        if number(&p, "expires")? <= now {
            return Err(Error::Expired);
        }
        let mut fields = Vec::new();
        for child in tx.children(&key)? {
            fields.push(inventory::read_field(&child)?);
        }
        let result = wire::Candidate {
            candidate_id: string(&p, "candidate")?,
            source: Some(wire::SourceIdentity {
                connection_id: connection.to_string(),
                object: Some(wire::SourceObject {
                    object_kind: string(&p, "object_kind")?,
                    object_id: string(&p, "object_id")?,
                    incarnation_evidence: p
                        .get("lifetime")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }),
            }),
            kind: number(&p, "kind")? as i32,
            observation: Some(wire::Observation {
                scan_id: Some(string(&p, "scan")?),
                evidence: Some(wire::Evidence {
                    source_id: string(&p, "source")?,
                    source_revision: p
                        .get("source_revision")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    observed_at_ms: number(&p, "observed")?,
                    valid_until_ms: number(&p, "valid")?,
                    fields,
                }),
            }),
        };
        tx.commit()?;
        Ok(result)
    }
}

fn update_review_scope(
    head: &mut Properties,
    before: &Usage,
    after: &Usage,
    policy: &Policy,
) -> Result<()> {
    let index = Class::Review as usize;
    let rows = number(head, "review_rows")?
        .checked_add(after.rows[index])
        .and_then(|v| v.checked_sub(before.rows[index]))
        .ok_or(Error::Corrupt)?;
    let bytes = number(head, "review_bytes")?
        .checked_add(after.bytes[index])
        .and_then(|v| v.checked_sub(before.bytes[index]))
        .ok_or(Error::Corrupt)?;
    if rows > policy.review_per_source.rows || bytes > policy.review_per_source.bytes {
        return Err(Error::Limit);
    }
    head.insert("review_rows".into(), int(rows)?);
    head.insert("review_bytes".into(), int(bytes)?);
    Ok(())
}

impl Tx<'_> {
    pub(super) fn review_removed(&mut self, connection: &str, rows: u64, bytes: u64) -> Result<()> {
        let key = head_key(connection);
        let mut p = self.get(&key)?.ok_or(Error::Corrupt)?;
        let rows = number(&p, "review_rows")?
            .checked_sub(rows)
            .ok_or(Error::Corrupt)?;
        let bytes = number(&p, "review_bytes")?
            .checked_sub(bytes)
            .ok_or(Error::Corrupt)?;
        p.insert("review_rows".into(), int(rows)?);
        p.insert("review_bytes".into(), int(bytes)?);
        operations::strip_envelope(&mut p);
        self.put(&key, "scans", Class::Checkpoint, p)?;
        Ok(())
    }
}
