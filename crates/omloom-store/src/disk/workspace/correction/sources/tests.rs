use super::*;

// An explicitly reviewed label wins over a broader prefix, in either order;
// unrelated descendants retain the existing prefix rewrite behavior.
#[test]
fn exact_source_choice_wins_over_earlier_prefix() {
    let mut mapping = api::IdentityMapping {
        sources: vec![
            api::IdentitySourceChoice {
                before: "Survey".into(),
                after: "A".into(),
            },
            api::IdentitySourceChoice {
                before: "Survey / routes".into(),
                after: "B".into(),
            },
        ],
        ..Default::default()
    };
    for _ in 0..2 {
        assert_eq!(renamed(&mapping, "Survey / routes"), "B");
        assert_eq!(renamed(&mapping, "Survey / interfaces"), "A / interfaces");
        assert_eq!(renamed(&mapping, "Other survey"), "Other survey");
        mapping.sources.reverse();
    }
}

// Nested label groups use the most specific reviewed prefix independently of
// request order, while partial segment names must not match that prefix.
#[test]
fn nested_source_uses_longest_matching_prefix_in_either_order() {
    let mut mapping = api::IdentityMapping {
        sources: vec![
            api::IdentitySourceChoice {
                before: "Survey".into(),
                after: "A".into(),
            },
            api::IdentitySourceChoice {
                before: "Survey / routes".into(),
                after: "B".into(),
            },
        ],
        ..Default::default()
    };
    for _ in 0..2 {
        assert_eq!(renamed(&mapping, "Survey / routes / leaf"), "B / leaf");
        assert_eq!(
            renamed(&mapping, "Survey / routes-extra / leaf"),
            "A / routes-extra / leaf"
        );
        mapping.sources.reverse();
    }
}
