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
        object_kind: "qemu".into(),
        object_id: "vm/101".into(),
        incarnation_evidence: Some("provider-generation-2".into()),
    };
    let observation = Observation {
        provenance: Some(Provenance {
            source_id: id(),
            source_revision: None,
            scan_id: Some(scan_id.clone()),
            observed_at_ms: 100,
            valid_until_ms: 200,
        }),
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
        candidates: vec![candidate.clone()],
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
        .provenance
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
    bad.connection_id = id();
    add(
        &mut cases,
        "foreign_scan_source",
        record::Body::Scan(bad),
        Some(Error::Reference),
    );
    let mut bad = scan;
    bad.candidates.push(candidate);
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
