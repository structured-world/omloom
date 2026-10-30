use omloom_contracts::{Error, Limits, MemoryBudget, decode_workspace, workspace as api};
use prost::Message;

// Omitted wire input cannot implicitly request an exclusive allocation.
#[test]
fn network_sharing_requires_an_explicit_mechanism() {
    let omitted = api::Network::decode([].as_slice()).expect("empty network");
    assert_eq!(omitted.sharing, api::SharedUse::Unspecified as i32);
    for mechanism in [
        api::SharedUse::Exclusive,
        api::SharedUse::Vrrp,
        api::SharedUse::Anycast,
    ] {
        let request = api::Network {
            sharing: mechanism as i32,
            ..Default::default()
        };
        let bytes = request.encode_to_vec();
        assert!(!bytes.is_empty());
        assert_eq!(
            api::Network::decode(bytes.as_slice()).expect("network"),
            request
        );
    }
}

// Reservations cover the decoded request until its owner, not just its wire buffer, drops.
#[test]
fn application_frame_has_owned_memory_and_strict_preflight() {
    let bytes = api::Request {
        working: None,
        command: Some(api::request::Command::Query(api::Query {
            limit: 25,
            search: "edge".into(),
            ..Default::default()
        })),
    }
    .encode_to_vec();
    let budget = MemoryBudget::new(16 * 1_048_576);
    let mut admitted = decode_workspace(&bytes, Limits::default(), &budget).expect("bounded frame");
    assert!(budget.used() > bytes.len());
    assert!(admitted.take().command.is_some());
    assert!(budget.used() > bytes.len());
    drop(admitted);
    assert_eq!(budget.used(), 0);
    assert!(matches!(
        decode_workspace(&bytes, Limits::default(), &MemoryBudget::new(1)),
        Err(Error::Limit)
    ));
    let mut duplicate = bytes.clone();
    duplicate.extend_from_slice(&bytes);
    assert!(matches!(
        decode_workspace(&duplicate, Limits::default(), &budget),
        Err(Error::DuplicateField)
    ));
    assert!(matches!(
        decode_workspace(&[0xfa, 0x03, 0], Limits::default(), &budget),
        Err(Error::UnknownField)
    ));
    assert!(matches!(
        decode_workspace(&[0x12, 0x7f], Limits::default(), &budget),
        Err(Error::Wire)
    ));
    assert_eq!(budget.used(), 0);
}

// Empty repeated messages still consume allocation/work capacity before prost can expand them.
#[test]
fn repeated_empty_messages_cannot_amplify_small_frames() {
    let bulk = api::Bulk {
        targets: vec![Default::default(); 4097],
        ..Default::default()
    };
    let bytes = api::Request {
        command: Some(api::request::Command::Bulk(bulk)),
        working: None,
    }
    .encode_to_vec();
    let budget = MemoryBudget::new(16 * 1_048_576);
    assert!(matches!(
        decode_workspace(&bytes, Limits::default(), &budget),
        Err(Error::Limit)
    ));
    assert_eq!(budget.used(), 0);
}

// Packed scalar references count individually before allocation, not once per buffer.
#[test]
fn requirements_bound_nodes_and_packed_members_before_decoding() {
    let budget = MemoryBudget::new(16 * 1_048_576);
    for (nodes, members) in [(65, 0), (1, 65)] {
        let node = api::RequirementNode {
            expression: Some(api::requirement_node::Expression::Group(
                api::RequirementGroup {
                    kind: api::RequirementGroupKind::All as i32,
                    members: vec![0; members],
                    minimum: 0,
                },
            )),
        };
        let bytes = api::Request {
            command: Some(api::request::Command::EvaluateRequirements(
                api::EvaluateRequirements {
                    nodes: vec![node; nodes],
                    ..Default::default()
                },
            )),
            working: None,
        }
        .encode_to_vec();
        assert!(matches!(
            decode_workspace(&bytes, Limits::default(), &budget),
            Err(Error::Limit)
        ));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn requirements_share_member_limits_across_packed_and_unpacked_segments() {
    // Splitting one repeated field across wire encodings cannot reset its allocation bound.
    fn message(tag: u32, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        prost::encoding::encode_key(tag, prost::encoding::WireType::LengthDelimited, &mut out);
        prost::encoding::encode_varint(body.len() as u64, &mut out);
        out.extend_from_slice(body);
        out
    }
    let budget = MemoryBudget::new(16 * 1_048_576);
    for (unpacked, extra) in [(true, 32), (true, 33), (false, 33)] {
        let mut group = vec![0x08, 1];
        group.extend(message(2, &[0; 32]));
        if unpacked {
            for _ in 0..extra {
                group.extend([0x10, 0]);
            }
        } else {
            group.extend(message(2, &vec![0; extra]));
        }
        let bytes = message(33, &message(2, &message(2, &group)));
        let result = decode_workspace(&bytes, Limits::default(), &budget);
        if extra == 32 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::Limit)));
        }
        drop(result);
        assert_eq!(budget.used(), 0);
    }
}
