//! Current judgments live with their owning plan step. Fixed-window counters
//! contain no resource identities and expire through ordinary detail cleanup.
use super::*;

pub(super) fn check_saved_feedback(
    plan: &api::PhysicalPlan,
    existing: Option<&api::PhysicalPlan>,
) -> Result<()> {
    for step in &plan.steps {
        if let Some(api::physical_step::Action::Replacement(replacement)) = &step.action {
            let prior = existing
                .and_then(|p| {
                    replacement_steps(p).find(|r| {
                        r.position == replacement.position
                            && r.previous == replacement.previous
                            && r.replacement == replacement.replacement
                            && r.source_position == replacement.source_position
                            && r.previous_binding == replacement.previous_binding
                            && r.previous_revision == replacement.previous_revision
                    })
                })
                .and_then(|r| r.feedback.as_ref());
            if replacement.feedback.as_ref() != prior {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(())
}

fn scope(context: &Context, working: &api::WorkingContext) -> String {
    // Read scope, not authorship or write privileges, determines disclosure.
    // The host supplies grants and their revision; callers select only roots.
    let mut hash = Sha256::new();
    hash.update(context.authority_revision.to_le_bytes());
    let mut grants: Vec<_> = context
        .grants
        .iter()
        .map(|grant| Sha256::digest(format!("{:?}/{:?}", grant.scope, grant.read).as_bytes()))
        .collect();
    grants.sort_unstable();
    grants.dedup();
    for grant in grants {
        hash.update(grant);
    }
    for root in &working.roots {
        hash.update(root.encode_to_vec());
    }
    format!("{:x}", hash.finalize())
}
fn parent(scope: &str) -> String {
    format!("replacement-quality/{scope}")
}
fn bucket(value: &api::PhysicalReplacementFeedback) -> String {
    format!(
        "{}/{}/{}/{}/{}/{}/{}",
        parent(&value.scope),
        value.until_ms,
        value.rule,
        value.component_kind,
        value.strength,
        value.decision,
        value.reason
    )
}
fn valid_decision(decision: i32, reason: i32) -> Result<()> {
    use api::{PhysicalReplacementDecision as D, PhysicalReplacementReason as R};
    match (D::try_from(decision), R::try_from(reason)) {
        (Ok(D::Confirmed | D::Rejected | D::Deferred), Ok(_)) => Ok(()),
        _ => Err(Error::Invalid),
    }
}
impl Tx<'_> {
    pub(super) fn reconcile_saved_feedback(
        &mut self,
        previous: &api::PhysicalPlan,
        current: &api::PhysicalPlan,
        now: u64,
    ) -> Result<()> {
        for replacement in replacement_steps(previous) {
            if let Some(feedback) = replacement.feedback.as_deref()
                && !replacement_steps(current)
                    .any(|value| value.feedback.as_deref() == Some(feedback))
            {
                self.quality_delta(feedback, false, now)?;
            }
        }
        Ok(())
    }
    fn replacement_hypothesis(
        &self,
        value: &api::PhysicalReplacement,
    ) -> Result<(api::PhysicalReplacementHypothesis, i32)> {
        let mut hash = Sha256::new();
        hash.update(b"replacement-rules-1");
        let mut component_kind = api::PhysicalComponentKind::Unspecified as i32;
        for reference in [
            value.position.as_ref(),
            value.previous.as_ref(),
            value.replacement.as_ref(),
            value.source_position.as_ref(),
        ] {
            if let Some(reference) = reference {
                hash.update(reference.encode_to_vec());
                let item = parse_target(Some(reference))?;
                hash.update(number(&self.asset(item)?, "revision")?.to_le_bytes());
                if let Some(definition) = self.physical_definition(item)? {
                    hash.update(definition.encode_to_vec());
                    if value.previous.as_ref() == Some(reference) {
                        component_kind = definition.component_kind;
                    }
                }
            } else {
                hash.update([0]);
            }
        }
        let same = value.previous.is_some() && value.previous == value.replacement;
        let binding_current = match self.replacement_basis(value) {
            Ok(Some(binding)) => {
                hash.update(binding.encode_to_vec());
                true
            }
            Ok(None) | Err(Error::Conflict | Error::Incomplete) => false,
            Err(error) => return Err(error),
        };
        let (strength, explanation) = if same {
            (
                api::PhysicalReplacementStrength::EstablishedIdentity as i32,
                "The selected asset identity is unchanged. Reseat or move is plausible; this does not prove attachment or service continuity.",
            )
        } else if binding_current && value.replacement.is_some() {
            (
                api::PhysicalReplacementStrength::Contextual as i32,
                "Distinct adopted assets and a current predecessor binding support a possible replacement. An addition, moved part or wrong predecessor remain alternatives. Model, name and IP are not identity proof.",
            )
        } else {
            (
                api::PhysicalReplacementStrength::Limited as i32,
                "Predecessor or current occupancy evidence is incomplete. Removal, source error, addition and replacement remain unresolved alternatives.",
            )
        };
        Ok((
            api::PhysicalReplacementHypothesis {
                basis: format!("{:x}", hash.finalize()),
                rule: 1,
                strength,
                explanation: explanation.into(),
            },
            component_kind,
        ))
    }
    fn quality_delta(
        &mut self,
        value: &api::PhysicalReplacementFeedback,
        add: bool,
        now: u64,
    ) -> Result<()> {
        if value.until_ms <= now {
            return Ok(());
        }
        let key = bucket(value);
        let old = self.get(&key)?;
        let count = old
            .as_ref()
            .map(|p| number(p, "count"))
            .transpose()?
            .unwrap_or(0);
        let count = if add {
            count.checked_add(1)
        } else {
            count.checked_sub(1)
        }
        .ok_or(Error::Corrupt)?;
        if count == 0 {
            self.delete(&key)?;
            return Ok(());
        }
        let at = value
            .until_ms
            .checked_sub(self.store.policy.detail_age_ms)
            .ok_or(Error::Corrupt)?;
        self.put_against(
            &key,
            &parent(&value.scope),
            Class::Detail,
            props([
                ("at", int(at)?),
                ("expires", int(value.until_ms)?),
                ("count", int(count)?),
                ("generation", int(u64::from(value.rule))?),
                ("kind", int(value.component_kind as u64)?),
                (
                    "expected",
                    int(u64::try_from(value.strength).map_err(|_| Error::Corrupt)?)?,
                ),
                ("state", int(value.decision as u64)?),
                ("vint", int(value.reason as u64)?),
            ]),
            old.as_ref(),
        )?;
        Ok(())
    }
    pub(super) fn review_replacement(
        &mut self,
        context: &Context,
        request: api::PhysicalReplacementReview,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<(api::PhysicalPlan, api::PhysicalReplacementHypothesis)> {
        let key = request.plan.as_ref().ok_or(Error::Invalid)?;
        let mut plan =
            self.load_physical_plan(context, key, working, request.decision.is_some())?;
        let step = plan
            .steps
            .get(request.step as usize)
            .ok_or(Error::Invalid)?;
        let Some(api::physical_step::Action::Replacement(replacement)) = &step.action else {
            return Err(Error::Invalid);
        };
        let (hypothesis, kind) = self.replacement_hypothesis(replacement)?;
        let Some(decision) = request.decision else {
            if request.basis.is_some() || request.reason != 0 {
                return Err(Error::Invalid);
            }
            return Ok((plan, hypothesis));
        };
        valid_decision(decision, request.reason)?;
        if key.revision == 0 || request.basis.as_ref() != Some(&hypothesis.basis) {
            return Err(Error::Conflict);
        }
        let scope = scope(context, working);
        let old = replacement.feedback.as_ref();
        if let Some(old) = old {
            if old.scope != scope {
                return Err(Error::Denied);
            }
            if old.until_ms <= now {
                return Err(Error::Expired);
            }
            if old.basis == hypothesis.basis
                && old.decision == decision
                && old.reason == request.reason
            {
                return Ok((plan, hypothesis));
            }
        }
        let duration = self.store.policy.detail_age_ms;
        let until = (now / duration)
            .checked_add(1)
            .and_then(|n| n.checked_mul(duration))
            .ok_or(Error::Limit)?;
        let feedback = api::PhysicalReplacementFeedback {
            decision,
            reason: request.reason,
            basis: hypothesis.basis.clone(),
            until_ms: old.map(|v| v.until_ms).unwrap_or(until),
            rule: hypothesis.rule,
            strength: hypothesis.strength,
            component_kind: kind,
            scope,
        };
        if let Some(old) = old {
            self.quality_delta(old, false, now)?;
        }
        self.quality_delta(&feedback, true, now)?;
        let Some(api::physical_step::Action::Replacement(replacement)) =
            &mut plan.steps[request.step as usize].action
        else {
            return Err(Error::Corrupt);
        };
        replacement.feedback = Some(Box::new(feedback));
        let step_key = format!("plan/{}/{}", key.id, request.step);
        let old = self.get(&step_key)?.ok_or(Error::Corrupt)?;
        let mut row = step_row(&plan.steps[request.step as usize])?;
        let class = if number(&old, "class")? == Class::Detail as u64 {
            row.insert("at".into(), int(number(&old, "at")?)?);
            Class::Detail
        } else {
            Class::Inventory
        };
        self.put_against(
            &step_key,
            &format!("plan/{}", key.id),
            class,
            row,
            Some(&old),
        )?;
        let header_key = format!("plan/{}", key.id);
        let old = self.get(&header_key)?.ok_or(Error::Corrupt)?;
        let mut row = old.clone();
        let class = if number(&row, "class")? == Class::Detail as u64 {
            Class::Detail
        } else {
            Class::Inventory
        };
        operations::strip_envelope(&mut row);
        plan.revision = number(&self.root, "revision")?
            .checked_add(1)
            .ok_or(Error::Limit)?;
        row.insert("revision".into(), int(plan.revision)?);
        self.put_against(&header_key, "plans", class, row, Some(&old))?;
        self.audit(
            &key.id,
            &context.actor,
            "replacement.reviewed",
            plan.revision,
            now,
        )?;
        Ok((plan, hypothesis))
    }
    pub(super) fn replacement_quality(
        &self,
        context: &Context,
        request: api::PhysicalReplacementQualityQuery,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<(Vec<api::PhysicalReplacementQuality>, Option<String>)> {
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        if request.after.as_ref().is_some_and(|s| s.len() > 256) {
            return Err(Error::Invalid);
        }
        let parent = parent(&scope(context, working));
        let rows = self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.expires>$now AND n.key>$after RETURN properties(n) AS p ORDER BY n.key LIMIT 26",
            params([("parent",text(parent)),("now",int(now)?),("after",text(request.after.as_deref().unwrap_or("")))]))?;
        let next = if rows.len() > 25 {
            Some(string(&rows[24], "key")?)
        } else {
            None
        };
        let mut output = Vec::with_capacity(rows.len().min(25));
        for row in rows.iter().take(25) {
            output.push(api::PhysicalReplacementQuality {
                component_kind: i32::try_from(number(row, "kind")?).map_err(|_| Error::Corrupt)?,
                rule: u32::try_from(number(row, "generation")?).map_err(|_| Error::Corrupt)?,
                strength: i32::try_from(number(row, "expected")?).map_err(|_| Error::Corrupt)?,
                decision: i32::try_from(number(row, "state")?).map_err(|_| Error::Corrupt)?,
                reason: i32::try_from(number(row, "vint")?).map_err(|_| Error::Corrupt)?,
                count: number(row, "count")?,
                until_ms: number(row, "expires")?,
            });
        }
        Ok((output, next))
    }
}
