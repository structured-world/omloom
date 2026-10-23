use omloom_contracts::{Error, ids::AssetId, wire::*};
use prost::Message;

pub struct Case {
    pub name: &'static str,
    pub bytes: Vec<u8>,
    pub error: Option<Error>,
}

// A real generated corpus: each invocation has independent UUIDs. Both codecs
// must preserve every value, including explicit clears and u64::MAX revisions.
pub fn cases() -> Vec<Case> {
    fn id() -> String {
        AssetId::generate().to_string()
    }
    fn add(cases: &mut Vec<Case>, name: &'static str, body: record::Body, error: Option<Error>) {
        cases.push(Case {
            name,
            bytes: Record { body: Some(body) }.encode_to_vec(),
            error,
        });
    }
    let mut cases = Vec::new();
    let target = AssetRef {
        asset_id: id(),
        incarnation: id(),
    };
    let scan_id = id();
    let source = SourceIdentity {
        connection_id: id(),
        object: Some(SourceObject {
            object_kind: "qemu".into(),
            object_id: "vm/101".into(),
            incarnation_evidence: Some("provider-generation-2".into()),
        }),
    };
    let observation = Observation {
        scan_id: Some(scan_id.clone()),
        evidence: Some(Evidence {
            source_id: id(),
            source_revision: None,
            observed_at_ms: 100,
            valid_until_ms: 200,
            fields: vec![
                Field {
                    name: "name".into(),
                    value: Some(field::Value::Text("сервер α".into())),
                },
                Field {
                    name: "description".into(),
                    value: Some(field::Value::Clear(NullValue::NullValue as i32)),
                },
                Field {
                    name: "enabled".into(),
                    value: Some(field::Value::Flag(false)),
                },
                Field {
                    name: "delta".into(),
                    value: Some(field::Value::Integer(i64::MIN)),
                },
            ],
        }),
    };
    let candidate = Candidate {
        candidate_id: id(),
        source: Some(source.clone()),
        kind: AssetKind::VirtualMachine as i32,
        observation: Some(observation.clone()),
    };
    let leaf = Requirement {
        kind: RequirementKind::Fact as i32,
        children: vec![],
        complete: false,
        resource: Some(target.clone()),
        fact: "storage.ready".into(),
    };
    let requirements = Requirements {
        version: 1,
        root: 1,
        nodes: vec![
            leaf.clone(),
            Requirement {
                kind: RequirementKind::All as i32,
                children: vec![0],
                complete: false,
                resource: None,
                fact: String::new(),
            },
        ],
    };
    let asset = Asset {
        target: Some(target.clone()),
        kind: AssetKind::VirtualMachine as i32,
        adoption: Adoption::Adopted as i32,
        revision: u64::MAX,
        intended: vec![IntendedField {
            field: Some(Field {
                name: "name".into(),
                value: Some(field::Value::Text("accepted intent".into())),
            }),
            authority_id: id(),
        }],
        imported: vec![observation.clone()],
        observed: vec![observation.clone()],
        sources: vec![source.clone()],
    };
    let operation = Operation {
        operation_id: id(),
        target: Some(target.clone()),
        inventory_revision: u64::MAX,
        plan_digest: vec![0xa5; 32],
        state: OperationState::Reconciling as i32,
        outcome: EffectOutcome::Unknown as i32,
        provider_task_id: Some("task:original".into()),
        revision: u64::MAX,
    };
    add(
        &mut cases,
        "asset",
        record::Body::Asset(asset.clone()),
        None,
    );
    add(
        &mut cases,
        "candidate",
        record::Body::Candidate(candidate.clone()),
        None,
    );
    let scan = ScanBatch {
        scan_id,
        connection_id: source.connection_id.clone(),
        prior_cursor: None,
        cursor: "page-2".into(),
        completeness: ScanCompleteness::SnapshotComplete as i32,
        snapshot_revision: Some("snapshot-1".into()),
        candidates: vec![ScanCandidate {
            candidate_id: candidate.candidate_id.clone(),
            source: source.object.clone(),
            kind: candidate.kind,
            evidence: observation.evidence.clone(),
        }],
    };
    add(&mut cases, "scan", record::Body::Scan(scan.clone()), None);
    add(
        &mut cases,
        "relation",
        record::Body::Relation(Relation {
            relation_id: id(),
            from: Some(target.clone()),
            to: Some(AssetRef {
                asset_id: id(),
                incarnation: id(),
            }),
            kind: RelationKind::StoredOn as i32,
            revision: 1,
            authority_id: id(),
        }),
        None,
    );
    add(
        &mut cases,
        "requirements",
        record::Body::Requirements(requirements.clone()),
        None,
    );
    add(
        &mut cases,
        "infrastructure_connection",
        record::Body::InfrastructureConnection(InfrastructureConnection {
            connection_id: id(),
            revision: 1,
            scope: Some(ConnectionScope {
                source: Some(target.clone()),
                destination: Some(target.clone()),
                direction: Direction::Forward as i32,
                vrf_id: Some(id()),
                vlan: Some(120),
                service: "tcp.443".into(),
            }),
            requirements: Some(requirements.clone()),
        }),
        None,
    );
    let provider = ProviderConnection {
        connection_id: id(),
        adapter: "proxmox".into(),
        endpoint: "https://pve.example.invalid:8006/".into(),
        trust_id: id(),
        credential_binding_id: None,
    };
    add(
        &mut cases,
        "provider_connection",
        record::Body::ProviderConnection(provider.clone()),
        None,
    );
    add(
        &mut cases,
        "operation",
        record::Body::Operation(operation.clone()),
        None,
    );
    // One current record can preserve omitted stale fields, refreshed false and
    // an explicit clear without duplicating an older version of any field.
    let mut partial = asset.clone();
    let mut fresh = observation.clone();
    let old_fields = &observation.evidence.as_ref().unwrap().fields;
    partial.observed[0].evidence.as_mut().unwrap().fields =
        vec![old_fields[0].clone(), old_fields[3].clone()];
    let fresh_evidence = fresh.evidence.as_mut().unwrap();
    fresh_evidence.fields = vec![old_fields[1].clone(), old_fields[2].clone()];
    fresh_evidence.observed_at_ms = 300;
    fresh_evidence.valid_until_ms = 400;
    fresh.scan_id = Some(id());
    partial.observed.push(fresh);
    add(
        &mut cases,
        "partial_refresh",
        record::Body::Asset(partial.clone()),
        None,
    );
    partial.imported = partial.observed.clone();
    add(
        &mut cases,
        "partial_import",
        record::Body::Asset(partial),
        None,
    );

    // Equal field names from genuinely distinct sources remain independently
    // attributed, even when they contradict the accepted intended value.
    let mut conflict = asset.clone();
    let mut other = observation.clone();
    let ev = other.evidence.as_mut().unwrap();
    ev.source_id = id();
    ev.fields.truncate(1);
    ev.fields[0].value = Some(field::Value::Text("other source".into()));
    conflict.observed.push(other);
    add(
        &mut cases,
        "conflicting_sources",
        record::Body::Asset(conflict),
        None,
    );

    // Shared descendants contribute to both branches; reachability admission
    // must not confuse a diamond DAG with a cycle or an unused definition.
    let diamond = Requirements {
        version: 1,
        root: 3,
        nodes: vec![
            leaf.clone(),
            Requirement {
                kind: RequirementKind::All as i32,
                children: vec![0],
                complete: true,
                ..Default::default()
            },
            Requirement {
                kind: RequirementKind::Any as i32,
                children: vec![0],
                complete: true,
                ..Default::default()
            },
            Requirement {
                kind: RequirementKind::All as i32,
                children: vec![1, 2],
                complete: true,
                ..Default::default()
            },
        ],
    };
    add(
        &mut cases,
        "requirement_diamond",
        record::Body::Requirements(diamond),
        None,
    );

    // The largest page still carries its connection and scan identity once.
    let mut page = scan.clone();
    let template = page.candidates[0].clone();
    page.candidates = (0..128)
        .map(|index| {
            let mut row = template.clone();
            row.candidate_id = id();
            row.source.as_mut().unwrap().object_id = index.to_string();
            row.evidence.as_mut().unwrap().source_id = id();
            row
        })
        .collect();
    add(&mut cases, "scan_full_page", record::Body::Scan(page), None);
    let mut identity_only = scan.clone();
    identity_only.candidates[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .clear();
    add(
        &mut cases,
        "scan_identity_only",
        record::Body::Scan(identity_only),
        None,
    );

    // A current layer cannot hide history by grouping older/newer values of
    // the same source/field under separate timestamps.
    let mut overlap = asset.clone();
    let mut newer = observation.clone();
    newer.evidence.as_mut().unwrap().observed_at_ms = 150;
    overlap.observed.push(newer);
    add(
        &mut cases,
        "overlapping_source_fields",
        record::Body::Asset(overlap),
        Some(Error::Duplicate),
    );
    let mut redundant = asset.clone();
    redundant.observed[0]
        .evidence
        .as_mut()
        .unwrap()
        .fields
        .truncate(1);
    let mut rest = observation.clone();
    rest.evidence.as_mut().unwrap().fields.remove(0);
    redundant.observed.push(rest);
    add(
        &mut cases,
        "redundant_provenance_group",
        record::Body::Asset(redundant),
        Some(Error::Duplicate),
    );
    let mut empty = asset.clone();
    empty.observed[0].evidence.as_mut().unwrap().fields.clear();
    add(
        &mut cases,
        "empty_current_evidence",
        record::Body::Asset(empty),
        Some(Error::Missing),
    );
    let mut orphan = requirements.clone();
    orphan.nodes.push(leaf);
    add(
        &mut cases,
        "orphan_fact",
        record::Body::Requirements(orphan),
        Some(Error::Reference),
    );
    let mut no_window = candidate.clone();
    no_window.observation.as_mut().unwrap().scan_id = None;
    add(
        &mut cases,
        "standalone_missing_window_identity",
        record::Body::Candidate(no_window),
        Some(Error::Missing),
    );
    let mut invalid = scan.clone();
    invalid.candidates[0]
        .evidence
        .as_mut()
        .unwrap()
        .valid_until_ms = 99;
    add(
        &mut cases,
        "scan_invalid_freshness",
        record::Body::Scan(invalid),
        Some(Error::Value),
    );
    let mut bad = asset.clone();
    bad.target.as_mut().unwrap().asset_id = "00000000-0000-0000-0000-000000000000".into();
    add(
        &mut cases,
        "nil_id",
        record::Body::Asset(bad),
        Some(Error::Identifier),
    );
    let mut bad = asset.clone();
    bad.kind = 999;
    add(
        &mut cases,
        "unknown_enum",
        record::Body::Asset(bad),
        Some(Error::Enumeration),
    );
    let mut bad = asset.clone();
    bad.intended.push(bad.intended[0].clone());
    add(
        &mut cases,
        "duplicate_intent",
        record::Body::Asset(bad),
        Some(Error::Duplicate),
    );
    let mut bad = candidate.clone();
    bad.source = None;
    add(
        &mut cases,
        "missing_source",
        record::Body::Candidate(bad),
        Some(Error::Missing),
    );
    let mut bad = candidate.clone();
    bad.observation
        .as_mut()
        .unwrap()
        .evidence
        .as_mut()
        .unwrap()
        .valid_until_ms = 99;
    add(
        &mut cases,
        "invalid_freshness",
        record::Body::Candidate(bad),
        Some(Error::Value),
    );
    let mut bad = scan.clone();
    bad.snapshot_revision = None;
    add(
        &mut cases,
        "false_scan_completion",
        record::Body::Scan(bad),
        Some(Error::Missing),
    );
    let mut bad = scan.clone();
    bad.candidates[0].source.as_mut().unwrap().object_kind = "invalid/kind".into();
    add(
        &mut cases,
        "invalid_scan_source",
        record::Body::Scan(bad),
        Some(Error::Value),
    );
    let mut bad = scan;
    bad.candidates.push(bad.candidates[0].clone());
    add(
        &mut cases,
        "duplicate_candidates",
        record::Body::Scan(bad),
        Some(Error::Duplicate),
    );
    let mut bad = requirements.clone();
    bad.nodes[1].children = vec![];
    add(
        &mut cases,
        "empty_group",
        record::Body::Requirements(bad),
        Some(Error::Value),
    );
    let mut bad = requirements.clone();
    bad.nodes[1].children = vec![7];
    add(
        &mut cases,
        "dangling_requirement",
        record::Body::Requirements(bad),
        Some(Error::Reference),
    );
    let mut bad = requirements.clone();
    bad.nodes[1].children = vec![1];
    add(
        &mut cases,
        "cyclic_requirement",
        record::Body::Requirements(bad),
        Some(Error::Cycle),
    );
    let mut bad = requirements;
    bad.nodes.push(Requirement {
        kind: RequirementKind::Any as i32,
        children: vec![2],
        complete: true,
        resource: None,
        fact: String::new(),
    });
    add(
        &mut cases,
        "unreachable_cycle",
        record::Body::Requirements(bad),
        Some(Error::Cycle),
    );
    let mut bad = operation.clone();
    bad.state = OperationState::Succeeded as i32;
    add(
        &mut cases,
        "unknown_is_not_success",
        record::Body::Operation(bad),
        Some(Error::Outcome),
    );
    let mut bad = operation;
    bad.state = OperationState::Cancelled as i32;
    add(
        &mut cases,
        "unknown_is_not_cancelled",
        record::Body::Operation(bad),
        Some(Error::Outcome),
    );
    let mut bad = provider.clone();
    bad.endpoint = "http://pve.example.invalid/".into();
    add(
        &mut cases,
        "insecure_endpoint",
        record::Body::ProviderConnection(bad),
        Some(Error::Value),
    );
    let mut bad = provider;
    bad.endpoint = "https://user:password@pve.example.invalid/".into();
    add(
        &mut cases,
        "credential_in_url",
        record::Body::ProviderConnection(bad),
        Some(Error::Value),
    );
    cases.push(Case {
        name: "missing_body",
        bytes: vec![],
        error: Some(Error::Missing),
    });
    cases.push(Case {
        name: "truncated_frame",
        bytes: vec![10, 127],
        error: Some(Error::Wire),
    });
    cases.push(Case {
        name: "unknown_field",
        bytes: vec![0x48, 1],
        error: Some(Error::UnknownField),
    });
    cases.push(Case {
        name: "overflow_varint",
        bytes: vec![0x80; 11],
        error: Some(Error::Wire),
    });
    cases.push(Case {
        name: "duplicate_body",
        bytes: [cases[0].bytes.as_slice(), cases[1].bytes.as_slice()].concat(),
        error: Some(Error::DuplicateField),
    });
    cases.push(Case {
        name: "invalid_utf8",
        bytes: vec![18, 3, 10, 1, 255],
        error: Some(Error::Wire),
    });
    cases.push(Case {
        name: "overflow_uint32",
        bytes: vec![66, 6, 8, 0x81, 0x80, 0x80, 0x80, 0x10],
        error: Some(Error::Wire),
    });
    cases.push(Case {
        name: "invalid_boolean",
        bytes: vec![66, 4, 26, 2, 24, 2],
        error: Some(Error::Wire),
    });
    cases.push(Case {
        name: "truncated_packed",
        bytes: vec![66, 5, 26, 3, 18, 1, 128],
        error: Some(Error::Wire),
    });
    cases
}
