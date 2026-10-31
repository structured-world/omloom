use super::*;
use knowledge::{Alternatives, Atom, Claim, Predicate, Quantity};

fn subject(id: &'static str) -> Subject<'static> {
    Subject {
        namespace: "local",
        resource: id,
        incarnation: "first",
        network: NetworkScope::Bound("lan"),
    }
}
fn leaf(id: &'static str, decision: Decision) -> Node<'static> {
    Node {
        subject: subject(id),
        role: Role::Requirement,
        member_role: MemberRole::Eligible,
        expression: Expression::Fact(decision),
    }
}
fn membership(complete: bool) -> Membership<'static> {
    Membership {
        complete,
        consensus: false,
        qualified_profile: None,
        revision: None,
    }
}
fn group<'a>(mode: Group, members: &'a [usize], membership: Membership<'a>) -> Node<'a> {
    Node {
        subject: subject("group"),
        role: Role::Requirement,
        member_role: MemberRole::Eligible,
        expression: Expression::Group {
            mode,
            members,
            membership,
        },
    }
}
fn run(nodes: &[Node<'_>]) -> Decision {
    let mut results = [Decision::unknown(Reason::NotSelected); MAX_NODES];
    evaluate(nodes, nodes.len() - 1, &mut results).expect("valid test graph")
}

#[test]
fn partial_all_any_and_fixed_threshold_preserve_unknown() {
    // Missing alternatives cannot establish failure; incomplete mandatory sets cannot pass.
    for complete in [false, true] {
        for mode in [Group::All, Group::Any, Group::AtLeast(2)] {
            let nodes = [
                leaf("a", Decision::PASS),
                leaf("b", Decision::unknown(Reason::Stale)),
                group(mode, &[0, 1], membership(complete)),
            ];
            let result = run(&nodes);
            assert_eq!(
                result.state,
                if matches!(mode, Group::Any) {
                    State::Satisfied
                } else {
                    State::Unknown
                }
            );
        }
        assert_eq!(
            run(&[
                leaf("a", Decision::FAIL),
                group(Group::All, &[0], membership(complete))
            ]),
            Decision::FAIL
        );
        assert_eq!(
            run(&[
                leaf("a", Decision::FAIL),
                group(Group::Any, &[0], membership(complete))
            ])
            .state,
            if complete {
                State::Failed
            } else {
                State::Unknown
            }
        );
    }
}

#[test]
fn duplicate_voters_learners_and_unqualified_majority_never_inflate_success() {
    // Three rows are not three votes when two describe the same incarnation.
    let nodes = [
        leaf("a", Decision::PASS),
        leaf("a", Decision::PASS),
        leaf("b", Decision::FAIL),
        group(Group::AtLeast(2), &[0, 1, 2], membership(true)),
    ];
    assert_eq!(run(&nodes), Decision::FAIL);
    let mut nodes = nodes;
    nodes[1].expression = Expression::Fact(Decision::FAIL);
    assert_eq!(run(&nodes).reason, Reason::Conflict);
    nodes[1].subject = subject("learner");
    nodes[1].member_role = MemberRole::Learner;
    nodes[3] = group(Group::Majority, &[0, 1, 2], membership(true));
    assert_eq!(run(&nodes).reason, Reason::Unqualified);
    let qualified = Membership {
        complete: true,
        consensus: true,
        qualified_profile: Some("provider/v1/voting"),
        revision: Some("membership/42"),
    };
    nodes[3] = group(Group::Majority, &[0, 1, 2], qualified);
    assert_eq!(run(&nodes), Decision::FAIL);
    nodes[2].expression = Expression::Fact(Decision::unknown(Reason::Restricted));
    assert_eq!(run(&nodes).state, State::Unknown);
    nodes[2].member_role = MemberRole::Unknown;
    assert_eq!(run(&nodes).reason, Reason::Incomplete);
}

#[test]
fn unselected_method_preferences_and_prose_cannot_change_mandatory_result() {
    // The selected action does not depend on a missing alternative method.
    let mut nodes = [
        leaf("selected", Decision::FAIL),
        leaf("other-method", Decision::unknown(Reason::Missing)),
        leaf("preference", Decision::PASS),
        group(Group::All, &[0, 2], membership(true)),
    ];
    nodes[2].role = Role::Preference;
    assert_eq!(run(&nodes), Decision::FAIL);
    nodes[0].expression = Expression::Fact(Decision::PASS);
    let mut results = [Decision::unknown(Reason::NotSelected); MAX_NODES];
    assert_eq!(evaluate(&nodes, 3, &mut results), Ok(Decision::PASS));
    assert_eq!(results[1].reason, Reason::NotSelected);
    nodes[0].role = Role::ConditionalBlock;
    assert_eq!(run(&nodes), Decision::FAIL);
}

#[test]
fn cycle_depth_empty_and_work_limits_are_enforced_without_recursion() {
    // A cycle or truncation cannot turn into a vacuously satisfied prerequisite.
    let nodes = [
        group(Group::All, &[1], membership(true)),
        group(Group::All, &[0], membership(true)),
    ];
    assert_eq!(run(&nodes).reason, Reason::Cycle);
    let mut output = [Decision::PASS; MAX_NODES];
    assert_eq!(
        evaluate(&[group(Group::All, &[], membership(true))], 0, &mut output),
        Err(Error::Invalid)
    );
    let indices: alloc::vec::Vec<_> = (1..=MAX_DEPTH).map(|i| [i]).collect();
    let mut nodes: alloc::vec::Vec<_> = indices
        .iter()
        .map(|m| group(Group::All, m, membership(true)))
        .collect();
    nodes.push(leaf("end", Decision::PASS));
    assert_eq!(evaluate(&nodes, 0, &mut output), Err(Error::Limit));
    let wide = [0usize; MAX_EDGES + 1];
    assert_eq!(
        evaluate(
            &[
                leaf("a", Decision::PASS),
                group(Group::All, &wide, membership(true))
            ],
            1,
            &mut output
        ),
        Err(Error::Limit)
    );
}

fn requirement(kind: EvidenceKind) -> Requirement<'static> {
    Requirement {
        subject: subject("guest"),
        semantic: "graceful-stop",
        vantage: "guest-network",
        phase: Phase::Current,
        kind,
        interval: None,
    }
}
fn evidence(wanted: Requirement<'static>) -> Evidence<'static> {
    Evidence {
        requirement: wanted,
        origin: Origin::DirectObservation,
        observed_at: Some(90),
        valid_until: Some(110),
        coverage: Some(Window {
            start: 80,
            end: 100,
        }),
        qualified: true,
        decision: Decision::PASS,
    }
}

#[test]
fn scope_vantage_phase_action_and_source_are_part_of_evidence_identity() {
    // Equal IP strings, provider success and forced power-off do not prove the requested guest action.
    let wanted = requirement(EvidenceKind::IndependentObservation);
    let mut source = evidence(wanted);
    source.requirement.subject.network = NetworkScope::Bound("other-lan");
    assert_eq!(fact(wanted, source, 100).reason, Reason::ScopeMismatch);
    source = evidence(wanted);
    source.requirement.vantage = "executor";
    assert_eq!(fact(wanted, source, 100).reason, Reason::VantageMismatch);
    source = evidence(wanted);
    source.origin = Origin::PeerAssertion;
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
    source = evidence(wanted);
    source.requirement.semantic = "force-off";
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
    source = evidence(wanted);
    source.requirement.phase = Phase::Predicted;
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
    source = evidence(wanted);
    source.origin = Origin::Accepted;
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
    source = evidence(wanted);
    source.qualified = false;
    assert_eq!(fact(wanted, source, 100).reason, Reason::Unqualified);
}

#[test]
fn interval_gaps_expiry_and_authority_remain_distinct() {
    // Missing coverage is not zero activity; inventory ownership is not a grant.
    let mut wanted = requirement(EvidenceKind::Observation);
    wanted.interval = Some(Window {
        start: 70,
        end: 100,
    });
    let source = evidence(wanted);
    assert_eq!(fact(wanted, source, 100).reason, Reason::Incomplete);
    assert_eq!(fact(wanted, source, 110).reason, Reason::Stale);
    let authority = Requirement {
        kind: EvidenceKind::Authority,
        ..wanted
    };
    assert_eq!(
        fact(authority, source, 100).reason,
        Reason::AuthorityUnavailable
    );
    let mut source = evidence(wanted);
    source.coverage = Some(Window {
        start: 60,
        end: 100,
    });
    assert_eq!(fact(wanted, source, 100), Decision::PASS);
}

struct Set {
    candidates: &'static [&'static str],
    excluded: &'static [&'static str],
    complete: bool,
}
impl Alternatives for Set {
    fn candidates(&self) -> usize {
        self.candidates.len()
    }
    fn candidate(&self, index: usize) -> Atom<'_> {
        Atom::Text(self.candidates[index])
    }
    fn exclusions(&self) -> usize {
        self.excluded.len()
    }
    fn exclusion(&self, index: usize) -> Atom<'_> {
        Atom::Text(self.excluded[index])
    }
    fn exhaustive(&self) -> bool {
        self.complete
    }
}
#[test]
fn partial_models_exclusions_and_independent_measurements_do_not_invent_fit() {
    // Neither a top-ranked model nor a score can certify rails; measured bounds can.
    let partial = Set {
        candidates: &["DL160"],
        excluded: &["DL120"],
        complete: false,
    };
    assert_eq!(
        knowledge::compare(
            Claim::Alternatives(&partial),
            Predicate::Is(Atom::Text("DL160"))
        )
        .state,
        State::Unknown
    );
    assert_eq!(
        knowledge::compare(
            Claim::Alternatives(&partial),
            Predicate::Is(Atom::Text("DL120"))
        ),
        Decision::FAIL
    );
    let complete = Set {
        candidates: &["DL160", "DL120"],
        excluded: &[],
        complete: true,
    };
    assert_eq!(
        knowledge::compare(
            Claim::Alternatives(&complete),
            Predicate::OneOf(&[Atom::Text("DL160"), Atom::Text("DL120")])
        ),
        Decision::PASS
    );
    let conflict = Set {
        candidates: &["DL160"],
        excluded: &["DL160"],
        complete: true,
    };
    assert_eq!(
        knowledge::compare(
            Claim::Alternatives(&conflict),
            Predicate::Is(Atom::Text("DL160"))
        )
        .reason,
        Reason::Conflict
    );
    for (width, expected) in [(480, State::Failed), (440, State::Satisfied)] {
        let claim: Claim<'_, Set> = Claim::Quantity(Quantity {
            lower: Some(width),
            upper: Some(width),
            unit: "mm",
            scale: 0,
        });
        assert_eq!(
            knowledge::compare(
                claim,
                Predicate::Maximum {
                    value: 4500,
                    unit: "mm",
                    scale: 1
                }
            )
            .state,
            expected
        );
    }
}

#[test]
fn console_viewing_does_not_prove_boot_disk_or_protection_control() {
    // These are conditional technical-control facts, never grants or compromise findings.
    for kind in ["vm", "device", "service", "container"] {
        let mut nodes = [
            leaf(kind, Decision::PASS),
            leaf("boot", Decision::unknown(Reason::Missing)),
            leaf("disk-writable", Decision::PASS),
            leaf("unlocked", Decision::PASS),
            group(Group::All, &[1, 2, 3], membership(true)),
        ];
        assert_eq!(run(&nodes).state, State::Unknown);
        nodes[1].expression = Expression::Fact(Decision::PASS);
        assert_eq!(run(&nodes), Decision::PASS);
        nodes[3].expression = Expression::Fact(Decision::unknown(Reason::Missing));
        assert_eq!(run(&nodes).state, State::Unknown);
    }
}

#[test]
fn requirements_unavailable_capability_is_not_missing_measurement() {
    // Unsupported/configuration/authority failures stay actionable without fabricated observations.
    let wanted = requirement(EvidenceKind::Observation);
    for reason in [
        Reason::Unsupported,
        Reason::Unconfigured,
        Reason::AuthorityUnavailable,
    ] {
        let mut source = evidence(wanted);
        source.qualified = false;
        source.observed_at = None;
        source.valid_until = None;
        source.decision = Decision::unknown(reason);
        assert_eq!(fact(wanted, source, 100), Decision::unknown(reason));
    }
}

#[test]
fn requirements_preferences_cannot_consume_a_real_voters_identity() {
    // Ignored annotations with the same subject cannot erase an eligible vote.
    let mut nodes = [
        leaf("a", Decision::PASS),
        leaf("a", Decision::PASS),
        group(Group::AtLeast(1), &[0, 1], membership(true)),
    ];
    nodes[0].role = Role::Preference;
    assert_eq!(run(&nodes), Decision::PASS);
}

#[test]
fn requirements_profile_match_never_supplies_missing_authority_or_facts() {
    // A tested artifact/version is not transitive to another provider release or method.
    let selected = Profile {
        method: "graceful-stop/v1",
        adapter_artifact: "qualified-artifact",
        provider_version: "supported-release",
    };
    assert_eq!(
        applicability(selected, Some(selected), Decision::PASS),
        Decision::PASS
    );
    for reason in [Reason::Missing, Reason::AuthorityUnavailable] {
        assert_eq!(
            applicability(selected, Some(selected), Decision::unknown(reason)),
            Decision::unknown(reason)
        );
    }
    let other = Profile {
        provider_version: "unqualified-release",
        ..selected
    };
    assert_eq!(
        applicability(other, Some(selected), Decision::PASS).reason,
        Reason::Unqualified
    );
    let other = Profile {
        method: "force-off/v1",
        ..selected
    };
    assert_eq!(
        applicability(other, Some(selected), Decision::PASS).reason,
        Reason::Unqualified
    );
}

#[test]
fn requirements_authority_evidence_cannot_prove_observed_service_state() {
    // Permission to reach a service is not an actual observation of that service.
    let wanted = requirement(EvidenceKind::Observation);
    let mut source = evidence(wanted);
    source.requirement.kind = EvidenceKind::Authority;
    source.origin = Origin::AuthorityDecision;
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
}

#[test]
fn requirements_unknown_network_binding_is_not_a_shared_network() {
    // Two missing network identifiers cannot establish a path in the same network.
    let mut wanted = requirement(EvidenceKind::Observation);
    wanted.subject.network = NetworkScope::Unknown;
    assert_eq!(
        fact(wanted, evidence(wanted), 100).reason,
        Reason::ScopeMismatch
    );
}

#[test]
fn requirements_do_not_expand_nonbinding_preferences() {
    // A cyclic advisory branch is outside the mandatory prerequisite graph.
    let mut preference = group(Group::All, &[1], membership(true));
    preference.role = Role::Preference;
    let nodes = [
        leaf("required", Decision::PASS),
        preference,
        group(Group::All, &[0, 1], membership(true)),
    ];
    assert_eq!(run(&nodes), Decision::PASS);
}

#[test]
fn requirements_observations_cannot_manufacture_accepted_intent() {
    // A discovered value does not mean an operator accepted that configuration.
    let wanted = requirement(EvidenceKind::Configuration);
    let mut source = evidence(wanted);
    source.requirement.kind = EvidenceKind::Observation;
    assert_eq!(fact(wanted, source, 100).reason, Reason::WrongEvidence);
}

#[test]
fn requirements_incomplete_unselected_groups_do_not_block_selected_facts() {
    // An unfinished alternative is not a prerequisite of the chosen method.
    for other in [
        group(Group::All, &[], membership(true)),
        group(Group::AtLeast(2), &[0], membership(true)),
    ] {
        let nodes = [leaf("chosen", Decision::PASS), other];
        let mut results = [Decision::unknown(Reason::NotSelected); MAX_NODES];
        assert_eq!(evaluate(&nodes, 0, &mut results), Ok(Decision::PASS));
        assert_eq!(results[1].reason, Reason::NotSelected);
        assert_eq!(evaluate(&nodes, 1, &mut results), Err(Error::Invalid));
    }
}
