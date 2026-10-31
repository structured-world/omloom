//! Shared prerequisite semantics. Inputs belong to a trusted fact producer;
//! a result never supplies authorization or evidence that an effect occurred.
pub mod knowledge;

pub const MAX_NODES: usize = 64;
pub const MAX_EDGES: usize = 256;
pub const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Satisfied,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Match,
    Mismatch,
    Missing,
    Stale,
    Conflict,
    Restricted,
    Unsupported,
    Unconfigured,
    AuthorityUnavailable,
    OwnerActionRequired,
    Incomplete,
    ScopeMismatch,
    VantageMismatch,
    Unqualified,
    WrongEvidence,
    Cycle,
    NotSelected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub state: State,
    pub reason: Reason,
}
impl Decision {
    pub const PASS: Self = Self {
        state: State::Satisfied,
        reason: Reason::Match,
    };
    pub const FAIL: Self = Self {
        state: State::Failed,
        reason: Reason::Mismatch,
    };
    pub const fn unknown(reason: Reason) -> Self {
        Self {
            state: State::Unknown,
            reason,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkScope<'a> {
    NotApplicable,
    Unknown,
    Bound(&'a str),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subject<'a> {
    pub namespace: &'a str,
    pub resource: &'a str,
    pub incarnation: &'a str,
    pub network: NetworkScope<'a>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Current,
    Predicted,
    ConcurrentIntent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Accepted,
    DirectObservation,
    PeerAssertion,
    AuthorityDecision,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    Configuration,
    Observation,
    IndependentObservation,
    Authority,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirement<'a> {
    pub subject: Subject<'a>,
    /// Includes semantic action and direction, not merely a transport port.
    pub semantic: &'a str,
    pub vantage: &'a str,
    pub phase: Phase,
    pub kind: EvidenceKind,
    pub interval: Option<Window>,
}
#[derive(Clone, Copy, Debug)]
pub struct Evidence<'a> {
    pub requirement: Requirement<'a>,
    pub origin: Origin,
    pub observed_at: Option<u64>,
    pub valid_until: Option<u64>,
    pub coverage: Option<Window>,
    /// Attribution and qualification are independent of the measured value.
    pub qualified: bool,
    pub decision: Decision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile<'a> {
    pub method: &'a str,
    pub adapter_artifact: &'a str,
    pub provider_version: &'a str,
}
/// Qualification binds the actual method/artifact/version. Applicability facts
/// remain separate from missing authority and from the operation's outcome.
pub fn applicability(
    selected: Profile<'_>,
    qualified: Option<Profile<'_>>,
    conditions: Decision,
) -> Decision {
    for value in [
        selected.method,
        selected.adapter_artifact,
        selected.provider_version,
    ] {
        if value.is_empty() || value.len() > 256 {
            return Decision::unknown(Reason::Unqualified);
        }
    }
    if qualified != Some(selected) {
        return Decision::unknown(Reason::Unqualified);
    }
    conditions
}

/// Validate provenance before interpreting the predicate's value. An adapter
/// cannot turn a host observation into a guest observation by changing its label.
pub fn fact(wanted: Requirement<'_>, evidence: Evidence<'_>, now: u64) -> Decision {
    if wanted.subject != evidence.requirement.subject
        || matches!(
            wanted.subject.network,
            NetworkScope::Unknown | NetworkScope::Bound("")
        )
    {
        return Decision::unknown(Reason::ScopeMismatch);
    }
    if wanted.semantic != evidence.requirement.semantic
        || wanted.phase != evidence.requirement.phase
    {
        return Decision::unknown(Reason::WrongEvidence);
    }
    if wanted.vantage != evidence.requirement.vantage {
        return Decision::unknown(Reason::VantageMismatch);
    }
    if matches!(
        evidence.decision.reason,
        Reason::Unsupported
            | Reason::Unconfigured
            | Reason::AuthorityUnavailable
            | Reason::Restricted
    ) {
        return Decision::unknown(evidence.decision.reason);
    }
    if !evidence.qualified {
        return Decision::unknown(Reason::Unqualified);
    }
    if wanted.kind == EvidenceKind::Configuration
        && (evidence.requirement.kind != EvidenceKind::Configuration
            || evidence.origin != Origin::Accepted)
    {
        return Decision::unknown(Reason::WrongEvidence);
    }
    if wanted.kind == EvidenceKind::Authority
        && (evidence.requirement.kind != EvidenceKind::Authority
            || evidence.origin != Origin::AuthorityDecision)
    {
        return Decision::unknown(Reason::AuthorityUnavailable);
    }
    if matches!(
        wanted.kind,
        EvidenceKind::Observation | EvidenceKind::IndependentObservation
    ) && (!matches!(
        evidence.origin,
        Origin::DirectObservation | Origin::PeerAssertion
    ) || !matches!(
        evidence.requirement.kind,
        EvidenceKind::Observation | EvidenceKind::IndependentObservation
    )) {
        return Decision::unknown(Reason::WrongEvidence);
    }
    if wanted.kind == EvidenceKind::IndependentObservation
        && evidence.origin != Origin::DirectObservation
    {
        return Decision::unknown(Reason::WrongEvidence);
    }
    if matches!(
        wanted.kind,
        EvidenceKind::Observation | EvidenceKind::IndependentObservation | EvidenceKind::Authority
    ) && (evidence.observed_at.is_none() || evidence.valid_until.is_none())
    {
        return Decision::unknown(Reason::Missing);
    }
    if evidence.observed_at.is_some_and(|at| at > now)
        || evidence.valid_until.is_some_and(|until| until <= now)
    {
        return Decision::unknown(Reason::Stale);
    }
    if let Some(interval) = wanted.interval {
        if interval.start >= interval.end || interval.end > now {
            return Decision::unknown(Reason::Incomplete);
        }
        match evidence.coverage {
            Some(coverage) if coverage.start <= interval.start && coverage.end >= interval.end => {}
            _ => return Decision::unknown(Reason::Incomplete),
        }
    }
    evidence.decision
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Requirement,
    Authority,
    ConditionalBlock,
    Preference,
    Fact,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberRole {
    Eligible,
    Learner,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub enum Group {
    All,
    Any,
    AtLeast(usize),
    Majority,
}
#[derive(Clone, Copy, Debug)]
pub struct Membership<'a> {
    pub complete: bool,
    /// Consensus needs a provider-qualified membership revision, not ping counts.
    pub consensus: bool,
    pub qualified_profile: Option<&'a str>,
    pub revision: Option<&'a str>,
}
#[derive(Clone, Copy, Debug)]
pub enum Expression<'a> {
    Fact(Decision),
    Group {
        mode: Group,
        members: &'a [usize],
        membership: Membership<'a>,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    pub subject: Subject<'a>,
    pub role: Role,
    pub member_role: MemberRole,
    pub expression: Expression<'a>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Limit,
    Invalid,
}
#[derive(Clone, Copy, Default)]
struct Frame {
    node: usize,
    child: usize,
}

/// Evaluate only the selected branch, once per node, with caller-owned output.
/// No allocation, recursion, clock read, storage access or hidden graph expansion.
pub fn evaluate(
    nodes: &[Node<'_>],
    root: usize,
    output: &mut [Decision],
) -> Result<Decision, Error> {
    if nodes.len() > MAX_NODES || output.len() < nodes.len() {
        return Err(Error::Limit);
    }
    if root >= nodes.len() {
        return Err(Error::Invalid);
    }
    let mut edges = 0;
    for node in nodes {
        if let Expression::Group { members, .. } = node.expression {
            if members.len() > MAX_EDGES - edges {
                return Err(Error::Limit);
            }
            edges += members.len();
            if members.iter().any(|&child| child >= nodes.len()) {
                return Err(Error::Invalid);
            }
        }
    }
    output[..nodes.len()].fill(Decision::unknown(Reason::NotSelected));
    let mut colors = [0u8; MAX_NODES];
    let mut stack = [Frame::default(); MAX_DEPTH];
    stack[0].node = root;
    colors[root] = 1;
    let mut depth = 1;
    while depth != 0 {
        let frame = &mut stack[depth - 1];
        let node = nodes[frame.node];
        // Global representation bounds remain checked above; unfinished alternatives
        // need semantic validation only when they become selected prerequisites.
        if frame.child == 0
            && let Expression::Group {
                mode,
                members,
                membership,
            } = node.expression
        {
            if members.is_empty() {
                return Err(Error::Invalid);
            }
            if let Group::AtLeast(k) = mode
                && (k == 0 || (membership.complete && k > members.len()))
            {
                return Err(Error::Invalid);
            }
        }
        if let Expression::Group { members, .. } = node.expression
            && frame.child < members.len()
        {
            let next = members[frame.child];
            frame.child += 1;
            if matches!(nodes[next].role, Role::Preference | Role::Fact) {
                continue;
            }
            if colors[next] == 1 {
                return Ok(Decision::unknown(Reason::Cycle));
            }
            if colors[next] == 0 {
                if depth == MAX_DEPTH {
                    return Err(Error::Limit);
                }
                stack[depth] = Frame {
                    node: next,
                    child: 0,
                };
                colors[next] = 1;
                depth += 1;
            }
            continue;
        }
        let mut result = match node.expression {
            Expression::Fact(decision) => decision,
            Expression::Group {
                mode,
                members,
                membership,
            } => aggregate(nodes, output, mode, members, membership)?,
        };
        if node.role == Role::ConditionalBlock {
            result = match result.state {
                State::Satisfied => Decision::FAIL,
                State::Failed => Decision::PASS,
                State::Unknown => result,
            };
        }
        output[frame.node] = result;
        colors[frame.node] = 2;
        depth -= 1;
    }
    Ok(output[root])
}

fn aggregate(
    nodes: &[Node<'_>],
    results: &[Decision],
    mode: Group,
    members: &[usize],
    membership: Membership<'_>,
) -> Result<Decision, Error> {
    let threshold = matches!(mode, Group::AtLeast(_) | Group::Majority);
    if (membership.consensus || matches!(mode, Group::Majority))
        && (!membership.consensus
            || !membership.complete
            || membership.qualified_profile.is_none_or(str::is_empty)
            || membership.revision.is_none_or(str::is_empty))
    {
        return Ok(Decision::unknown(Reason::Unqualified));
    }
    let mut pass = 0;
    let mut fail = 0;
    let mut unknown = 0;
    let mut reason = Reason::Incomplete;
    for (position, &index) in members.iter().enumerate() {
        let node = nodes[index];
        if matches!(node.role, Role::Preference | Role::Fact) {
            continue;
        }
        if threshold {
            if node.member_role == MemberRole::Learner {
                continue;
            }
            let mut duplicate = false;
            for &previous in &members[..position] {
                if matches!(nodes[previous].role, Role::Preference | Role::Fact)
                    || nodes[previous].member_role == MemberRole::Learner
                {
                    continue;
                }
                if nodes[previous].subject == node.subject {
                    if results[previous] != results[index]
                        || nodes[previous].member_role != node.member_role
                    {
                        return Ok(Decision::unknown(Reason::Conflict));
                    }
                    duplicate = true;
                    break;
                }
            }
            if duplicate {
                continue;
            }
        }
        let decision = if threshold && node.member_role == MemberRole::Unknown {
            Decision::unknown(Reason::Incomplete)
        } else {
            results[index]
        };
        match decision.state {
            State::Satisfied => pass += 1,
            State::Failed => fail += 1,
            State::Unknown => {
                unknown += 1;
                reason = decision.reason;
            }
        }
    }
    let count = pass + fail + unknown;
    if count == 0 {
        return Err(Error::Invalid);
    }
    match mode {
        Group::All if fail != 0 => Ok(Decision::FAIL),
        Group::All if unknown == 0 && membership.complete => Ok(Decision::PASS),
        Group::Any if pass != 0 => Ok(Decision::PASS),
        Group::Any if unknown == 0 && membership.complete => Ok(Decision::FAIL),
        Group::AtLeast(k) if pass >= k => Ok(Decision::PASS),
        Group::AtLeast(k) if membership.complete && pass + unknown < k => Ok(Decision::FAIL),
        Group::Majority
            if unknown != 0
                && members
                    .iter()
                    .any(|&i| nodes[i].member_role == MemberRole::Unknown) =>
        {
            Ok(Decision::unknown(Reason::Incomplete))
        }
        Group::Majority if pass > count / 2 => Ok(Decision::PASS),
        Group::Majority if pass + unknown <= count / 2 => Ok(Decision::FAIL),
        _ => Ok(Decision::unknown(reason)),
    }
}

#[cfg(test)]
mod tests;
