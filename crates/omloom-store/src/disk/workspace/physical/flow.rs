//! Domain projection over the shared plan dependency and requirement mechanisms.
use super::*;
use crate::plans as core;

pub(super) struct Analysis {
    pub states: Vec<core::Readiness>,
    pub ancestors: [u64; core::MAX_STEPS],
}
pub(super) fn analysis(steps: &[api::PhysicalStep]) -> Result<Analysis> {
    if steps.len() > core::MAX_STEPS {
        return Err(Error::Limit);
    }
    let mut edges = 0;
    let mut dependencies = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let source = step
            .control
            .as_ref()
            .map(|c| c.after.as_slice())
            .unwrap_or(&[]);
        if source.len() > core::MAX_EDGES - edges {
            return Err(Error::Limit);
        }
        let mut derived = None;
        if let Some(api::physical_step::Action::Wiring(wire)) = &step.action
            && let Some(api::physical_wiring::Basis::AfterStep(previous)) = wire.basis
        {
            let previous = previous as usize;
            if previous >= index {
                return Err(Error::Invalid);
            }
            let Some(api::physical_step::Action::Wiring(prior)) = &steps[previous].action else {
                return Err(Error::Invalid);
            };
            if prior.cable != wire.cable || prior.end_key != wire.end_key {
                return Err(Error::Invalid);
            }
            for intervening in &steps[previous + 1..index] {
                if let Some(api::physical_step::Action::Wiring(intervening)) = &intervening.action
                    && intervening.cable == wire.cable
                    && intervening.end_key == wire.end_key
                {
                    return Err(Error::Invalid);
                }
            }
            derived = Some(previous);
            for dependency in source {
                if dependency.step as usize == previous {
                    derived = None;
                    break;
                }
            }
        }
        let count = source.len() + usize::from(derived.is_some());
        if count > core::MAX_EDGES - edges {
            return Err(Error::Limit);
        }
        edges += count;
        let mut after = Vec::with_capacity(count);
        for dependency in source {
            let trigger =
                match api::PlanTrigger::try_from(dependency.when).map_err(|_| Error::Invalid)? {
                    api::PlanTrigger::Reported => core::Trigger::Reported,
                    api::PlanTrigger::Satisfied => core::Trigger::Satisfied,
                    api::PlanTrigger::Failed => core::Trigger::Failed,
                    _ => return Err(Error::Invalid),
                };
            if dependency.step as usize >= steps.len() {
                return Err(Error::Invalid);
            }
            if trigger != core::Trigger::Reported
                && steps[dependency.step as usize]
                    .control
                    .as_ref()
                    .and_then(|c| c.check.as_ref())
                    .is_none()
            {
                return Err(Error::Invalid);
            }
            after.push(core::Dependency {
                step: dependency.step as usize,
                trigger,
            });
        }
        if let Some(step) = derived {
            after.push(core::Dependency {
                step,
                trigger: core::Trigger::Reported,
            });
        }
        dependencies.push(after);
    }
    let mut nodes = Vec::with_capacity(steps.len());
    for (step, after) in steps.iter().zip(&dependencies) {
        let outcome =
            match api::PhysicalStepState::try_from(step.state).map_err(|_| Error::Invalid)? {
                api::PhysicalStepState::Recorded => core::Outcome::Reported,
                api::PhysicalStepState::NotPerformed => core::Outcome::NotPerformed,
                _ => core::Outcome::Pending,
            };
        let check = step
            .checked_state
            .map(|state| -> Result<crate::requirements::Decision> {
                Ok(crate::requirements::Decision {
                    state: match api::RequirementState::try_from(state)
                        .map_err(|_| Error::Corrupt)?
                    {
                        api::RequirementState::Satisfied => crate::requirements::State::Satisfied,
                        api::RequirementState::Failed => crate::requirements::State::Failed,
                        _ => crate::requirements::State::Unknown,
                    },
                    reason: crate::requirements::Reason::Incomplete,
                })
            })
            .transpose()?;
        nodes.push(core::Step {
            after,
            outcome,
            check,
        });
    }
    let mut output = vec![core::Readiness::Waiting; steps.len()];
    let mut ancestors = [0u64; core::MAX_STEPS];
    core::assess(&nodes, &mut output, Some(&mut ancestors)).map_err(|e| match e {
        core::Invalid::Limit => Error::Limit,
        _ => Error::Invalid,
    })?;
    Ok(Analysis {
        states: output,
        ancestors,
    })
}

impl Tx<'_> {
    pub(super) fn refresh_plan_control(
        &self,
        context: &Context,
        plan: &mut api::PhysicalPlan,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<()> {
        for step in &mut plan.steps {
            step.checked_state = None;
            step.checked_reason = None;
            if step.state != api::PhysicalStepState::Recorded as i32 {
                continue;
            }
            if let Some(check) = step.control.as_ref().and_then(|c| c.check.as_ref()) {
                let result = match self.evaluate_requirements_in_phase(
                    context,
                    check.clone(),
                    working,
                    now,
                    crate::requirements::Phase::PostEffect,
                ) {
                    Ok(value) => value,
                    Err(Error::Denied | Error::Missing | Error::Conflict) => {
                        api::RequirementEvaluation {
                            state: api::RequirementState::Unknown as i32,
                            reason: api::RequirementReason::Restricted as i32,
                            ..Default::default()
                        }
                    }
                    Err(Error::Limit) => api::RequirementEvaluation {
                        state: api::RequirementState::Unknown as i32,
                        reason: api::RequirementReason::Incomplete as i32,
                        ..Default::default()
                    },
                    Err(error) => return Err(error),
                };
                step.checked_state = Some(result.state);
                step.checked_reason = Some(result.reason);
            }
        }
        let states = analysis(&plan.steps)?.states;
        for (step, state) in plan.steps.iter_mut().zip(states) {
            step.readiness = match state {
                core::Readiness::Ready => api::PlanReadiness::Ready,
                core::Readiness::Waiting => api::PlanReadiness::Waiting,
                core::Readiness::Unselected => api::PlanReadiness::Unselected,
                core::Readiness::Complete => api::PlanReadiness::Complete,
            } as i32;
        }
        Ok(())
    }
}
