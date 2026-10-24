use super::*;

enum Change<'a> {
    Dispatch,
    Evidence {
        state: OperationState,
        outcome: EffectOutcome,
        provider_task: Option<&'a str>,
    },
}

fn digest(plan: &Plan<'_>, target: &Properties) -> Result<[u8; 32]> {
    bounded(plan.caller)?;
    bounded(plan.retry_key)?;
    if !token(plan.action) || plan.arguments.len() > 64 {
        return Err(Error::Invalid);
    }
    let mut sorted = BTreeMap::new();
    for field in plan.arguments {
        if !token(&field.name) || sorted.insert(&field.name, field).is_some() {
            return Err(Error::Invalid);
        }
        match &field.value {
            Some(field::Value::Text(v)) if v.len() <= 4096 => {}
            Some(field::Value::Integer(_) | field::Value::Flag(_) | field::Value::Clear(0)) => {}
            _ => return Err(Error::Invalid),
        }
    }
    let mut hash = Sha256::new();
    hash.update(b"omloom/accepted-plan/v1");
    for value in [
        target
            .get("asset")
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?,
        target
            .get("incarnation")
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?,
        plan.caller,
        plan.action,
    ] {
        hash_part(&mut hash, value.as_bytes());
    }
    hash.update(plan.inventory_revision.to_be_bytes());
    hash.update(plan.expires_at_ms.to_be_bytes());
    for (_, field) in sorted {
        hash_field(&mut hash, field)?;
    }
    Ok(hash.finalize().into())
}
fn checkpoint(plan: &Plan<'_>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"omloom/retry/v1");
    hash.update((plan.caller.len() as u64).to_be_bytes());
    hash.update(plan.caller);
    hash.update(plan.retry_key);
    format!("retry/{}", hex(&hash.finalize()))
}
pub(super) fn state(p: &Properties) -> Result<OperationState> {
    OperationState::try_from(number(p, "state")? as i32).map_err(|_| Error::Corrupt)
}
fn outcome(p: &Properties) -> Result<EffectOutcome> {
    EffectOutcome::try_from(number(p, "outcome")? as i32).map_err(|_| Error::Corrupt)
}
pub(super) fn terminal(s: OperationState) -> bool {
    matches!(
        s,
        OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
    )
}
fn lease_key(target: AssetRef) -> String {
    format!("lease/{}/{}", target.asset, target.incarnation)
}

impl Store {
    /// Persist confirmed arguments, retry checkpoint, initial work and audit in
    /// one transaction. Reusing a key with a changed plan never creates work.
    pub fn accept(&self, plan: &Plan<'_>, now: u64) -> Result<OperationId> {
        int(now)?;
        int(plan.expires_at_ms)?;
        if now >= plan.expires_at_ms {
            return Err(Error::Expired);
        }
        let target_fields = target_props(plan.target);
        let hash = digest(plan, &target_fields)?;
        let checkpoint = checkpoint(plan);
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        if let Some(saved) = tx.get(&checkpoint)? {
            if now >= number(&saved, "expires")? {
                return Err(Error::Expired);
            }
            if stored_digest(&saved)? != hash {
                return Err(Error::Conflict);
            }
            return string(&saved, "operation")?
                .parse()
                .map_err(|_| Error::Corrupt);
        }
        let asset = tx.asset(plan.target)?;
        if number(&asset, "revision")? != plan.inventory_revision
            || number(&asset, "adoption")? != wire::Adoption::Adopted as u64
        {
            return Err(Error::Conflict);
        }
        let id = OperationId::generate();
        let key = operation_key(id);
        let mut operation = target_fields;
        operation.extend(props([
            ("operation", text(id.to_string())),
            ("caller", text(plan.caller)),
            ("action", text(plan.action)),
            ("inventory_revision", int(plan.inventory_revision)?),
            ("digest", digest_value(hash)),
            ("state", int(OperationState::Queued as u64)?),
            ("outcome", int(EffectOutcome::NotSent as u64)?),
            ("revision", int(1)?),
            ("at", int(now)?),
        ]));
        tx.put(&key, "operations", Class::Pending, operation)?;
        for field in plan.arguments {
            let p = inventory::field_props(field)?;
            tx.put(
                &format!("{key}/argument/{}", field.name),
                &key,
                Class::Pending,
                p,
            )?;
        }
        tx.put(
            &checkpoint,
            "checkpoints",
            Class::Checkpoint,
            props([
                ("operation", text(id.to_string())),
                ("digest", digest_value(hash)),
                ("expires", int(plan.expires_at_ms)?),
            ]),
        )?;
        // This outbox is the local durable work consumer, not future webhooks.
        tx.put(
            &format!("work/{id}"),
            "work",
            Class::Work,
            props([("operation", text(id.to_string()))]),
        )?;
        tx.audit(&key, plan.caller, "operation.accepted", 1, now)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn operation(&self, id: OperationId) -> Result<DurableOperation> {
        let tx = self.begin()?;
        let key = operation_key(id);
        let p = tx.get(&key)?.ok_or(Error::Missing)?;
        let mut arguments = Vec::new();
        for row in tx.children(&key)? {
            if row.contains_key("field") {
                arguments.push(inventory::read_field(&row)?);
            }
        }
        let result = DurableOperation {
            id,
            target: target(&p)?,
            state: state(&p)?,
            outcome: outcome(&p)?,
            revision: number(&p, "revision")?,
            provider_task: p
                .get("provider_task")
                .and_then(Value::as_str)
                .map(str::to_owned),
            action: string(&p, "action")?,
            caller: string(&p, "caller")?,
            inventory_revision: number(&p, "inventory_revision")?,
            plan_digest: stored_digest(&p)?,
            arguments,
        };
        tx.commit()?;
        Ok(result)
    }

    /// Claiming unknown in-flight work is reconciliation only. It cannot reset
    /// effect knowledge or turn an expired lease into permission to resend.
    pub fn claim(&self, id: OperationId, worker: &str, now: u64, until: u64) -> Result<Lease> {
        bounded(worker)?;
        int(now)?;
        int(until)?;
        if until <= now {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let key = operation_key(id);
        let mut operation = tx.get(&key)?.ok_or(Error::Missing)?;
        if terminal(state(&operation)?) {
            return Err(Error::UnsafeTransition);
        }
        let target = target(&operation)?;
        let lease_key = lease_key(target);
        let prior = tx.get(&lease_key)?;
        if let Some(p) = &prior {
            if number(p, "until")? > now {
                return Err(Error::Conflict);
            }
            let previous = string(p, "operation")?;
            if previous != id.to_string() {
                if let Some(in_flight) = tx.get(&format!("operation/{previous}"))? {
                    if outcome(&in_flight)? == EffectOutcome::Unknown {
                        return Err(Error::Conflict);
                    }
                } else {
                    // Missing detail is not proof that a previously admitted
                    // effect resolved. Only a settled lease checkpoint is.
                    if p.get("settled") != Some(&Value::Bool(true)) {
                        return Err(Error::Conflict);
                    }
                }
            }
        }
        let epoch = prior
            .as_ref()
            .map(|p| number(p, "epoch"))
            .transpose()?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        tx.put(
            &lease_key,
            "leases",
            Class::Checkpoint,
            props([
                ("operation", text(id.to_string())),
                ("worker", text(worker)),
                ("epoch", int(epoch)?),
                ("until", int(until)?),
                ("settled", Value::Bool(false)),
            ]),
        )?;
        if outcome(&operation)? == EffectOutcome::Unknown {
            operation.insert("state".into(), int(OperationState::Reconciling as u64)?);
            let revision = number(&operation, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?;
            operation.insert("revision".into(), int(revision)?);
            strip_envelope(&mut operation);
            tx.put(&key, "operations", Class::Pending, operation)?;
            tx.audit(&key, worker, "operation.reconciling", revision, now)?;
        }
        tx.commit()?;
        Ok(Lease {
            operation: id,
            worker: worker.into(),
            epoch,
            until_ms: until,
        })
    }

    /// Commit the uncertainty marker before the executor can perform external I/O.
    /// The caller still owns current SID authorization and provider preconditions.
    pub fn prepare_dispatch(&self, lease: &Lease, expected: u64, now: u64) -> Result<u64> {
        self.transition(lease, expected, Change::Dispatch, now)
    }
    /// Record provider evidence under the current fence. This never sends I/O.
    pub fn reconcile(
        &self,
        lease: &Lease,
        expected: u64,
        next: OperationState,
        effect: EffectOutcome,
        provider_task: Option<&str>,
        now: u64,
    ) -> Result<u64> {
        self.transition(
            lease,
            expected,
            Change::Evidence {
                state: next,
                outcome: effect,
                provider_task,
            },
            now,
        )
    }
    fn transition(
        &self,
        lease: &Lease,
        expected: u64,
        change: Change<'_>,
        now: u64,
    ) -> Result<u64> {
        let (next, effect, provider_task, dispatch) = match change {
            Change::Dispatch => (OperationState::Running, EffectOutcome::Unknown, None, true),
            Change::Evidence {
                state,
                outcome,
                provider_task,
            } => (state, outcome, provider_task, false),
        };
        int(now)?;
        if let Some(task) = provider_task {
            bounded(task)?;
        }
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let key = operation_key(lease.operation);
        let mut operation = tx.get(&key)?.ok_or(Error::Missing)?;
        let lease_key = lease_key(target(&operation)?);
        let persisted = tx.get(&lease_key)?.ok_or(Error::Conflict)?;
        if number(&persisted, "epoch")? != lease.epoch
            || persisted.get("operation") != Some(&text(lease.operation.to_string()))
            || persisted.get("worker") != Some(&text(&lease.worker))
            || number(&persisted, "until")? <= now
            || number(&operation, "revision")? != expected
        {
            return Err(Error::Conflict);
        }
        let old = state(&operation)?;
        let old_effect = outcome(&operation)?;
        if let Some(task) = provider_task
            && let Some(saved) = operation.get("provider_task")
            && saved.as_str().ok_or(Error::Corrupt)? != task
        {
            return Err(Error::Conflict);
        }
        if terminal(old) {
            return Err(Error::UnsafeTransition);
        }
        if dispatch {
            if old != OperationState::Queued || old_effect != EffectOutcome::NotSent {
                return Err(Error::UnsafeTransition);
            }
            let asset = tx.asset(target(&operation)?)?;
            if number(&asset, "revision")? != number(&operation, "inventory_revision")? {
                return Err(Error::Conflict);
            }
        } else {
            let valid = match next {
                OperationState::Reconciling | OperationState::NeedsAttention => {
                    effect == EffectOutcome::Unknown && old_effect == EffectOutcome::Unknown
                }
                OperationState::Succeeded => {
                    effect == EffectOutcome::Confirmed && old_effect == EffectOutcome::Unknown
                }
                OperationState::Failed => {
                    effect == EffectOutcome::ConfirmedNoEffect
                        && old_effect == EffectOutcome::Unknown
                }
                OperationState::Cancelled => {
                    effect == EffectOutcome::NotSent
                        && old_effect == EffectOutcome::NotSent
                        && old == OperationState::Queued
                }
                _ => false,
            };
            if !valid {
                return Err(Error::UnsafeTransition);
            }
        }
        let revision = expected.checked_add(1).ok_or(Error::Limit)?;
        operation.insert("state".into(), int(next as u64)?);
        operation.insert("outcome".into(), int(effect as u64)?);
        operation.insert("revision".into(), int(revision)?);
        operation.insert("at".into(), int(now)?);
        if let Some(task) = provider_task {
            operation.insert("provider_task".into(), text(task));
        }
        strip_envelope(&mut operation);
        tx.put(
            &key,
            "operations",
            if terminal(next) {
                Class::Detail
            } else {
                Class::Pending
            },
            operation,
        )?;
        tx.audit(&key, &lease.worker, "operation.transition", revision, now)?;
        if terminal(next) {
            let mut settled = persisted;
            settled.insert("settled".into(), Value::Bool(true));
            strip_envelope(&mut settled);
            tx.put(&lease_key, "leases", Class::Checkpoint, settled)?;
            tx.delete(&format!("work/{}", lease.operation))?;
            // Terminal arguments no longer serve recovery; keep the summary and
            // retry checkpoint, rather than retaining the whole accepted request.
            for p in tx.children(&key)? {
                if p.contains_key("field") {
                    tx.delete(&string(&p, "key")?)?;
                }
            }
        }
        tx.commit()?;
        Ok(revision)
    }

    /// One bounded durable-work page, ordered by operation ID. Resume after the
    /// last returned ID even if that operation remains unresolved or is removed.
    /// Restart at None after reaching an empty page to discover newly queued work.
    /// A returned ID is not a dispatch grant.
    pub fn pending(&self, after: Option<OperationId>) -> Result<Vec<OperationId>> {
        let tx = self.begin()?;
        let after = after.map_or_else(String::new, |id| format!("work/{id}"));
        let rows=tx.query("MATCH (n:OmRow) WHERE n.parent='work' AND n.key>$after RETURN properties(n) AS p, n.key AS ordering ORDER BY ordering LIMIT $limit",params([("after",text(after)),("limit",int(u64::from(self.policy.batch_rows))?)]))?;
        let result = rows
            .iter()
            .map(|p| string(p, "operation")?.parse().map_err(|_| Error::Corrupt))
            .collect::<Result<_>>()?;
        tx.commit()?;
        Ok(result)
    }
}

pub(super) fn strip_envelope(p: &mut Properties) {
    for key in ["key", "parent", "class", "size"] {
        p.remove(key);
    }
}
