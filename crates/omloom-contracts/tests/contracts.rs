mod support;
use omloom_contracts::{Error, Limits, MemoryBudget, decode, ids::*};

#[test]
fn serialization_and_error_corpus() {
    // Every domain record and malformed scenario must match its exact error,
    // roundtrip without field loss, and return its reservation on drop.
    for case in support::cases() {
        let budget = MemoryBudget::new(16 * 1_048_576);
        let result = decode(&case.bytes, Limits::default(), &budget);
        match case.error {
            Some(error) => assert_eq!(result.err(), Some(error), "{}", case.name),
            None => {
                let value = result.unwrap_or_else(|e| panic!("{}: {e}", case.name));
                let bytes = value.encode().unwrap();
                assert_eq!(bytes.as_ref(), case.bytes, "{}", case.name);
                assert!(budget.used() > 0);
            }
        }
        assert_eq!(budget.used(), 0, "{}", case.name);
    }
}

#[test]
fn missing_incarnation_evidence_remains_reviewable() {
    // An adapter may not know the lifetime of a discovered object. Preserve
    // that candidate for review instead of forcing invented identity evidence;
    // an accepted provider mapping must still have qualified lifetime evidence.
    use omloom_contracts::wire::{Record, record};
    use prost::Message;
    let cases = support::cases();
    let mut candidate = Record::decode(cases[1].bytes.as_slice()).unwrap();
    let Some(record::Body::Candidate(value)) = &mut candidate.body else {
        panic!("candidate fixture")
    };
    value
        .source
        .as_mut()
        .unwrap()
        .object
        .as_mut()
        .unwrap()
        .incarnation_evidence = None;
    let budget = MemoryBudget::new(16 * 1_048_576);
    assert!(decode(&candidate.encode_to_vec(), Limits::default(), &budget).is_ok());
    let mut asset = Record::decode(cases[0].bytes.as_slice()).unwrap();
    let Some(record::Body::Asset(value)) = &mut asset.body else {
        panic!("asset fixture")
    };
    value.sources[0]
        .object
        .as_mut()
        .unwrap()
        .incarnation_evidence = None;
    assert_eq!(
        decode(&asset.encode_to_vec(), Limits::default(), &budget).err(),
        Some(Error::Missing)
    );
}

#[test]
fn hard_limits_apply_before_decode_and_can_only_be_tightened() {
    // A caller cannot raise hard frame/field/depth limits; insufficient memory
    // must fail before the generated parser creates attacker-controlled values.
    let budget = MemoryBudget::new(0);
    let bytes = &support::cases()[0].bytes;
    assert_eq!(
        decode(bytes, Limits::default(), &budget).err(),
        Some(Error::Limit)
    );
    assert_eq!(budget.used(), 0);
    let budget = MemoryBudget::new(16 * 1_048_576);
    for limits in [
        Limits {
            bytes: 0,
            ..Limits::default()
        },
        Limits {
            fields: 0,
            ..Limits::default()
        },
        Limits {
            depth: 1,
            ..Limits::default()
        },
    ] {
        assert_eq!(decode(bytes, limits, &budget).err(), Some(Error::Limit));
        assert_eq!(budget.used(), 0);
    }
    let oversized = vec![0; 1_048_577];
    assert_eq!(
        decode(
            &oversized,
            Limits {
                bytes: usize::MAX,
                fields: usize::MAX,
                depth: usize::MAX
            },
            &budget
        )
        .err(),
        Some(Error::Limit)
    );
}

#[test]
fn memory_reservation_follows_the_value_and_encoded_copy() {
    // Concurrent retained values cannot reuse each other's allocation allowance.
    let case = &support::cases()[0];
    let budget = MemoryBudget::new(16 * 1_048_576);
    let record = decode(&case.bytes, Limits::default(), &budget).unwrap();
    let charge = budget.used();
    let encoded = record.encode().unwrap();
    assert!(budget.used() > charge);
    drop(record);
    assert_eq!(budget.used(), encoded.as_ref().len());
    drop(encoded);
    assert_eq!(budget.used(), 0);
    let small = MemoryBudget::new(charge);
    let record = decode(&case.bytes, Limits::default(), &small).unwrap();
    assert_eq!(
        decode(&case.bytes, Limits::default(), &small).err(),
        Some(Error::Limit)
    );
    drop(record);
    assert_eq!(small.used(), 0);
}

#[test]
fn canonical_nominal_identifiers() {
    // Accept only the documented lowercase UUIDv7 form. Shape does not grant
    // authority, and different nominal types cannot be substituted implicitly.
    let generated = AssetId::generate();
    assert_eq!(generated.to_string().parse::<AssetId>().unwrap(), generated);
    let uuid = generated.to_string();
    assert!(IncarnationId::parse(&uuid).is_ok());
    assert!(AssetId::parse(&uuid.replace('-', "")).is_err());
    assert!(AssetId::parse(&format!("urn:uuid:{uuid}")).is_err());
    assert!(AssetId::parse("00000000-0000-0000-0000-000000000000").is_err());
    let mut wrong = uuid.into_bytes();
    wrong[14] = b'4';
    assert!(AssetId::parse(std::str::from_utf8(&wrong).unwrap()).is_err());
}

#[test]
fn requirement_boundary_and_deep_cycle_are_stack_safe() {
    // The maximum-depth dependency chain uses a flat graph. A cycle at its
    // far end must reject without stack recursion or trusting a visited prefix.
    use omloom_contracts::wire::*;
    use prost::Message;
    let target = AssetRef {
        asset_id: AssetId::generate().to_string(),
        incarnation: IncarnationId::generate().to_string(),
    };
    let mut nodes = vec![Requirement {
        kind: RequirementKind::Fact as i32,
        resource: Some(target),
        fact: "ready".into(),
        ..Default::default()
    }];
    for index in 1..256 {
        nodes.push(Requirement {
            kind: RequirementKind::All as i32,
            children: vec![index - 1],
            complete: true,
            ..Default::default()
        });
    }
    let mut graph = Requirements {
        version: 1,
        root: 255,
        nodes,
    };
    let budget = MemoryBudget::new(16 * 1_048_576);
    let encoded = Record {
        body: Some(record::Body::Requirements(graph.clone())),
    }
    .encode_to_vec();
    assert!(decode(&encoded, Limits::default(), &budget).is_ok());
    graph.nodes[1].children = vec![255];
    let encoded = Record {
        body: Some(record::Body::Requirements(graph)),
    }
    .encode_to_vec();
    assert_eq!(
        decode(&encoded, Limits::default(), &budget).err(),
        Some(Error::Cycle)
    );
}

#[test]
fn repeated_empty_messages_cannot_bypass_work_limits() {
    // Empty repeated messages still cost allocations. Reject before prost
    // allocates the repeated container, even if semantic validation would fail.
    use omloom_contracts::wire::*;
    use prost::Message;
    let asset = Asset {
        observed: vec![Observation::default(); 4096],
        ..Default::default()
    };
    let bytes = Record {
        body: Some(record::Body::Asset(asset)),
    }
    .encode_to_vec();
    let budget = MemoryBudget::new(16 * 1_048_576);
    assert_eq!(
        decode(&bytes, Limits::default(), &budget).err(),
        Some(Error::Limit)
    );
    assert_eq!(budget.used(), 0);
}

#[test]
fn concurrent_admissions_share_and_release_the_budget() {
    // Multiple consumers of one budget cannot collectively exceed it. Release
    // survives both successful validation and rejected attempts on other threads.
    let bytes = &support::cases()[0].bytes;
    let budget = MemoryBudget::new(16 * 1_048_576);
    let value = decode(bytes, Limits::default(), &budget).unwrap();
    let charge = budget.used();
    drop(value);
    let budget = MemoryBudget::new(charge);
    std::thread::scope(|scope| {
        let first = decode(bytes, Limits::default(), &budget).unwrap();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let budget = &budget;
            handles.push(scope.spawn(move || {
                assert_eq!(
                    decode(bytes, Limits::default(), budget).err(),
                    Some(Error::Limit)
                )
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(budget.used(), charge);
        drop(first);
    });
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_field_windows_survive_partial_refresh() {
    // A partial refresh of B cannot freshen or discard omitted A. The current
    // asset must carry both independent windows without storing earlier samples.
    use omloom_contracts::wire::{Record, record};
    use prost::Message;
    let mut wire = Record::decode(support::cases()[0].bytes.as_slice()).unwrap();
    let Some(record::Body::Asset(asset)) = &mut wire.body else {
        panic!("asset fixture")
    };
    let mut fresh = asset.observed[0].clone();
    asset.observed[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .truncate(1);
    let ev = fresh.evidence.as_mut().unwrap();
    ev.fields = vec![ev.fields[2].clone()];
    ev.observed_at_ms = 300;
    ev.valid_until_ms = 400;
    asset.observed.push(fresh);
    let budget = MemoryBudget::new(16 * 1_048_576);
    let bytes = wire.encode_to_vec();
    let admitted = decode(&bytes, Limits::default(), &budget).unwrap();
    assert_eq!(admitted.encode().unwrap().as_ref(), bytes);
    let Some(record::Body::Asset(asset)) = &admitted.wire().body else {
        panic!("asset fixture")
    };
    assert_eq!(
        asset.observed[0].evidence.as_ref().unwrap().valid_until_ms,
        200
    );
    assert_eq!(
        asset.observed[1].evidence.as_ref().unwrap().observed_at_ms,
        300
    );
}

#[test]
fn unreachable_requirement_nodes_are_rejected() {
    // A valid but unused fact has no consumer under this expression's root.
    // Reject it instead of carrying it into confirmation, evaluation or storage.
    use omloom_contracts::wire::{Record, record};
    use prost::Message;
    let case = support::cases()
        .into_iter()
        .find(|c| c.name == "requirements")
        .unwrap();
    let mut wire = Record::decode(case.bytes.as_slice()).unwrap();
    let Some(record::Body::Requirements(graph)) = &mut wire.body else {
        panic!("requirements fixture")
    };
    graph.nodes.push(graph.nodes[0].clone());
    let budget = MemoryBudget::new(16 * 1_048_576);
    assert_eq!(
        decode(&wire.encode_to_vec(), Limits::default(), &budget).err(),
        Some(Error::Reference)
    );
    assert_eq!(budget.used(), 0);
}

#[test]
fn scan_context_is_encoded_once_per_batch() {
    // A full page must carry its connection and scan context once, including
    // when every row is otherwise a separately identified review candidate.
    use omloom_contracts::wire::{Record, record};
    use prost::Message;
    let case = support::cases()
        .into_iter()
        .find(|c| c.name == "scan")
        .unwrap();
    let mut wire = Record::decode(case.bytes.as_slice()).unwrap();
    let Some(record::Body::Scan(scan)) = &mut wire.body else {
        panic!("scan fixture")
    };
    let template = scan.candidates[0].clone();
    scan.candidates = (0..128)
        .map(|index| {
            let mut row = template.clone();
            row.candidate_id = CandidateId::generate().to_string();
            row.source.as_mut().unwrap().object_id = index.to_string();
            row.evidence.as_mut().unwrap().source_id = SourceId::generate().to_string();
            row
        })
        .collect();
    let connection = scan.connection_id.clone();
    let scan_id = scan.scan_id.clone();
    let bytes = wire.encode_to_vec();
    let budget = MemoryBudget::new(16 * 1_048_576);
    let admitted = decode(&bytes, Limits::default(), &budget).unwrap();
    assert_eq!(admitted.encode().unwrap().as_ref(), bytes);
    for context in [connection, scan_id] {
        assert_eq!(
            bytes
                .windows(context.len())
                .filter(|part| *part == context.as_bytes())
                .count(),
            1
        );
    }
}

#[test]
fn shared_requirement_frontier_obeys_the_edge_ceiling() {
    // A dense, root-reachable DAG reaches the exact edge bound with shared
    // leaves. Enqueue-once reachability must preserve it and reject edge 1025.
    use omloom_contracts::wire::*;
    use prost::Message;
    let target = AssetRef {
        asset_id: AssetId::generate().to_string(),
        incarnation: IncarnationId::generate().to_string(),
    };
    let mut nodes: Vec<_> = (0..64)
        .map(|index| Requirement {
            kind: RequirementKind::Fact as i32,
            resource: Some(target.clone()),
            fact: format!("ready.{index}"),
            ..Default::default()
        })
        .collect();
    for index in 0..16 {
        nodes.push(Requirement {
            kind: RequirementKind::Any as i32,
            children: (0..if index == 15 { 48 } else { 64 }).collect(),
            complete: true,
            ..Default::default()
        });
    }
    nodes.push(Requirement {
        kind: RequirementKind::All as i32,
        children: (64..80).collect(),
        complete: true,
        ..Default::default()
    });
    let graph = Requirements {
        version: 1,
        root: 80,
        nodes,
    };
    let budget = MemoryBudget::new(16 * 1_048_576);
    let bytes = Record {
        body: Some(record::Body::Requirements(graph.clone())),
    }
    .encode_to_vec();
    assert!(decode(&bytes, Limits::default(), &budget).is_ok());
    let mut excess = graph;
    excess.nodes[79].children.push(48);
    let bytes = Record {
        body: Some(record::Body::Requirements(excess)),
    }
    .encode_to_vec();
    assert_eq!(
        decode(&bytes, Limits::default(), &budget).err(),
        Some(Error::Limit)
    );
}
