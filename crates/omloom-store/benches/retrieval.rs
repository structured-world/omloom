use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use omloom_contracts::{
    Limits, MemoryBudget,
    ids::*,
    wire::{self, field, record},
};
use omloom_store::{Budget, CLASSES, Policy, Store};
use prost::Message;
use std::{hint::black_box, time::Duration};

fn retrieval(c: &mut Criterion) {
    let mut group = c.benchmark_group("current_inventory");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(250))
        .measurement_time(Duration::from_secs(1));
    group.throughput(Throughput::Elements(1));
    for scale in [20, 200] {
        let directory = tempfile::tempdir().expect("benchmark directory");
        let policy = Policy {
            classes: [Budget {
                rows: 20_000,
                bytes: 32 * 1_048_576,
            }; CLASSES],
            total_bytes: 128 * 1_048_576,
            detail_age_ms: 1000,
            physical_reservation_max_ms: 1000,
            review_age_ms: 500,
            review_per_source: Budget {
                rows: 512,
                bytes: 2 * 1_048_576,
            },
            batch_rows: 128,
        };
        let store = Store::open(directory.path(), policy).expect("open benchmark store");
        let mut sample = None;
        for _ in 0..scale {
            let target = AssetRef {
                asset: AssetId::generate(),
                incarnation: IncarnationId::generate(),
            };
            let asset = wire::Asset {
                target: Some(wire::AssetRef {
                    asset_id: target.asset.to_string(),
                    incarnation: target.incarnation.to_string(),
                }),
                kind: wire::AssetKind::Device as i32,
                adoption: wire::Adoption::Adopted as i32,
                revision: 1,
                intended: vec![wire::IntendedField {
                    authority_id: AuthorityId::generate().to_string(),
                    field: Some(wire::Field {
                        name: "name".into(),
                        value: Some(field::Value::Text("benchmark-host".into())),
                    }),
                }],
                observed: vec![wire::Observation {
                    scan_id: Some(ScanId::generate().to_string()),
                    evidence: Some(wire::Evidence {
                        source_id: SourceId::generate().to_string(),
                        source_revision: None,
                        observed_at_ms: 1,
                        valid_until_ms: 1000,
                        fields: (0..4)
                            .map(|index| wire::Field {
                                name: format!("field_{index}"),
                                value: Some(field::Value::Text("x".repeat(256))),
                            })
                            .collect(),
                    }),
                }],
                ..Default::default()
            };
            let bytes = wire::Record {
                body: Some(record::Body::Asset(asset)),
            }
            .encode_to_vec();
            let record = omloom_contracts::decode(
                &bytes,
                Limits::default(),
                &MemoryBudget::new(32 * 1_048_576),
            )
            .expect("admit benchmark record");
            store
                .save_asset(&record, None, "benchmark", 1)
                .expect("seed benchmark asset");
            sample = Some((target, record));
        }
        let (target, record) = sample.expect("nonempty benchmark scale");
        let before = store.usage().expect("initial usage");
        group.bench_with_input(BenchmarkId::new("get", scale), &target, |b, target| {
            b.iter(|| black_box(store.asset(*target).expect("current read")))
        });
        group.bench_function(BenchmarkId::new("unchanged_input", scale), |b| {
            b.iter(|| {
                black_box(
                    store
                        .save_asset(&record, Some(1), "benchmark", 2)
                        .expect("identical input"),
                )
            })
        });
        assert_eq!(
            store.usage().expect("final usage"),
            before,
            "reads and unchanged input must not persist state"
        );
    }
    group.finish();
}
fn equipment_claim(c: &mut Criterion) {
    // Measure bounded semantic comparison inputs, including order normalization.
    let mut group = c.benchmark_group("equipment_claim");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(250));
    for count in [1, 16] {
        for reverse in [false, true] {
            let mut alternatives = wire::EquipmentAlternatives {
                candidates: (0..count)
                    .map(|i| wire::EquipmentAlternative {
                        value: format!("Candidate-{i:02}"),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            };
            if reverse {
                alternatives.candidates.reverse();
            }
            let claim = wire::EquipmentClaim {
                basis: "Reviewed chassis label".into(),
                assertion: Some(wire::equipment_claim::Assertion::Alternatives(alternatives)),
            };
            group.bench_with_input(
                BenchmarkId::new(if reverse { "reordered" } else { "canonical" }, count),
                &claim,
                |b, value| {
                    b.iter(|| {
                        omloom_contracts::equipment::encode(black_box(value))
                            .expect("valid bounded claim")
                    })
                },
            );
        }
    }
    group.finish();
}
fn host_source_options(c: &mut Criterion) {
    use omloom_contracts::workspace as api;
    use omloom_store::workspace::*;
    let mut group = c.benchmark_group("host_source_options");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(250));
    for count in [1, 32, 126] {
        let directory = tempfile::tempdir().expect("benchmark directory");
        let store = Store::open(
            directory.path(),
            Policy {
                classes: [Budget {
                    rows: 20_000,
                    bytes: 32 * 1_048_576,
                }; CLASSES],
                total_bytes: 128 * 1_048_576,
                detail_age_ms: 1000,
                physical_reservation_max_ms: 1000,
                review_age_ms: 500,
                review_per_source: Budget {
                    rows: 512,
                    bytes: 2 * 1_048_576,
                },
                batch_rows: 128,
            },
        )
        .expect("benchmark store");
        let actor = Context {
            actor: "benchmark".into(),
            fixture: "benchmark".into(),
            grants: vec![Grant {
                scope: Scope::All,
                read: Fields::All,
                write: Fields::All,
                writable: true,
            }],
            teams: Vec::new(),
            recipients: Vec::new(),
            schema_admin: false,
            authority_revision: 1,
        };
        let field = |name: &str| wire::Field {
            name: "name".into(),
            value: Some(field::Value::Text(name.into())),
        };
        let mut report = api::HostReport {
            version: 1,
            platform: "linux".into(),
            source_basis: "a".repeat(64),
            boot_basis: "b".repeat(64),
            observed_at: 100,
            findings: vec![
                api::HostFinding {
                    key: "hardware".into(),
                    kind: wire::AssetKind::Device as i32,
                    fields: vec![field("Benchmark host")],
                    ..Default::default()
                },
                api::HostFinding {
                    key: "installation".into(),
                    kind: wire::AssetKind::SoftwareInstallation as i32,
                    parent: Some("hardware".into()),
                    fields: vec![field("Benchmark OS")],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        for index in 0..count {
            report.findings.push(api::HostFinding {
                key: format!("interface/{index}"),
                kind: wire::AssetKind::Interface as i32,
                parent: Some("installation".into()),
                fields: vec![field(&format!("eth{index}"))],
                ..Default::default()
            });
        }
        let execute = |command| {
            store
                .workspace(
                    &actor,
                    api::Request {
                        command: Some(command),
                        working: Some(api::WorkingContext::default()),
                    },
                    100,
                )
                .expect("benchmark request")
                .result
                .expect("benchmark reply")
        };
        let api::reply::Result::Preview(review) =
            execute(api::request::Command::HostImport(api::HostImport {
                source: "Benchmark source".into(),
                selection: report
                    .findings
                    .iter()
                    .map(|node| api::HostSelection {
                        key: node.key.clone(),
                        ..Default::default()
                    })
                    .collect(),
                report: Some(Box::new(report.clone())),
                ..Default::default()
            }))
        else {
            panic!("preview required");
        };
        let api::reply::Result::Mutation(accepted) =
            execute(api::request::Command::HostImport(api::HostImport {
                preview: review.id,
                ..Default::default()
            }))
        else {
            panic!("acceptance required");
        };
        let request = api::HostSources {
            host: Some(accepted.targets[0].clone()),
            report: Some(Box::new(report)),
            ..Default::default()
        };
        let before = store.usage().expect("usage");
        group.bench_with_input(
            BenchmarkId::new("interfaces", count),
            &request,
            |b, request| {
                b.iter(|| black_box(execute(api::request::Command::HostSources(request.clone()))));
            },
        );
        assert_eq!(
            before,
            store.usage().expect("usage"),
            "source options must not write"
        );
    }
    group.finish();
}
criterion_group!(benches, retrieval, equipment_claim, host_source_options);
criterion_main!(benches);
