use super::*;

fn report() -> api::HostReport {
    api::HostReport {
        version: 1,
        boot_basis: "test-boot".into(),
        ..Default::default()
    }
}

// Generation class and validity are independent; temporary literals must never
// enter the serialized report while deprecated stable facts stay explicit.
#[test]
fn mac_addresses_filter_temporary_and_keep_deprecated() {
    let values=native::parse_mac_interfaces("en0: flags=1<UP> mtu 1500\n\tinet 192.0.2.1 netmask 0xffffff00\n\tinet6 2001:db8::1 prefixlen 64 temporary\n\tinet6 2001:db8::2 prefixlen 64 deprecated\n").expect("valid interface fixture");
    assert_eq!(values[0].addresses.len(), 2);
    assert_eq!(
        values[0].addresses[1].state,
        api::AddressState::Deprecated as i32
    );
    assert!(!format!("{values:?}").contains("2001:db8::1"));
}
// Process identity uses the source boot/PID, not a shared display name.
#[test]
fn equally_named_processes_remain_separate_findings() {
    let mut value = report();
    native::parse_lsof(
        "p1\ncserver\nf1\ntIPv4\nn*:80\nTST=LISTEN\np2\ncserver\nf2\ntIPv4\nn*:81\nTST=LISTEN\n",
        &mut value,
    )
    .expect("valid listener fixture");
    assert_eq!(value.findings.len(), 2);
    assert_ne!(value.findings[0].key, value.findings[1].key);
    assert_eq!(
        value.binds[0].value.as_ref().expect("socket fact").protocol,
        "tcp"
    );
}

// lsof includes unbound UDP descriptors as *:*. They are not endpoints and
// cannot invalidate actual listeners or produce a fictitious port zero.
#[test]
fn unbound_udp_descriptor_does_not_hide_listeners() {
    let mut value = report();
    native::parse_lsof(
        "p1\ncserver\nf1\ntIPv4\nn*:*\nf2\ntIPv4\nn*:80\nTST=LISTEN\n",
        &mut value,
    )
    .expect("unbound descriptor is not a listener");
    assert_eq!(value.binds.len(), 1);
    assert_eq!(value.binds[0].value.as_ref().expect("listener").port, 80);
}

// Kernel-only rows remain useful evidence, but never fabricate an owning service.
#[test]
fn kernel_and_process_tables_preserve_unresolved_and_dual_stack_binds() {
    let mut value = report();
    native::parse_mac_sockets("Proto Recv-Q Send-Q Local Address Foreign Address (state)\ntcp46 0 0 *.443 *.* LISTEN\ntcp4 0 0 127.0.0.1.8000 *.* LISTEN\ntcp4 0 0 192.0.2.1.22 192.0.2.2.444 ESTABLISHED\n", "tcp", &mut value).expect("kernel TCP table");
    native::parse_mac_sockets("Proto Recv-Q Send-Q Local Address Foreign Address (state)\nudp6 0 0 *.5353 *.*\nudp4 0 0 192.0.2.1.123 192.0.2.2.123\n", "udp", &mut value).expect("kernel UDP table");
    assert_eq!(value.binds.len(), 2);
    assert!(value.findings.is_empty());
    assert!(value.binds.iter().all(|v| v.owner.is_empty()));
    assert_eq!(
        value.binds[0].value.as_ref().expect("bind").family,
        "IPv4+IPv6"
    );
}

// ss can read a socket whose process is hidden from this OS account. Preserve
// its family-specific bind without guessing a service or IPv4 dual-stack support.
#[test]
fn linux_kernel_only_binds_keep_family_and_unknown_owner() {
    let mut value = report();
    native::parse_ss(
        "tcp LISTEN 0 4096 *:443 *:*\nudp UNCONN 0 0 [::]:5353 [::]:*\n",
        "IPv6",
        &mut value,
    )
    .expect("accessible kernel facts");
    assert_eq!(value.binds.len(), 2);
    assert!(value.findings.is_empty());
    assert!(
        value
            .binds
            .iter()
            .all(|v| v.owner.is_empty() && v.value.as_ref().expect("bind").family == "IPv6")
    );
}
// Decoder work is refused before constructing a deeply nested JSON object tree.
#[test]
fn structured_input_has_a_stack_safety_ceiling() {
    assert!(native::json(&format!("{}0{}", "[".repeat(17), "]".repeat(17))).is_err());
    assert!(native::json("{\"value\":\"[[[\"}").is_ok());
}

// A file that consumes the whole RPC work budget leaves no room for scope and
// reviewed matches. Refuse it at production rather than publish an unusable file.
#[test]
fn report_reserves_work_for_the_review_request_envelope() {
    let mut value = report();
    for index in 0..128 {
        let mut node = finding(
            &format!("hardware/{index}"),
            wire::AssetKind::Device,
            None,
            "Machine",
        );
        for name in [
            "description",
            "purpose",
            "supplier",
            "model",
            "purchase_reference",
            "contract_reference",
            "disposal_reference",
        ] {
            node.fields.push(text(name, "Selected reference"));
        }
        value.findings.push(node);
    }
    assert!(encode(value).is_err());
}

// ALF paths can contain spaces; a basename must never replace the rule subject.
#[test]
fn application_rules_preserve_identity_and_refuse_incomplete_rows() {
    let rows = native::parse_alf_apps("Total number of apps = 2\n1 : /Applications/A B.app\n (Allow incoming connections)\n2 : /usr/bin/test\n (Block incoming connections)\n").expect("complete ALF source");
    assert_eq!(rows[0].subject, "/Applications/A B.app");
    assert_eq!(rows[1].action, "block incoming");
    assert_eq!(
        native::parse_alf_apps(
            "Total number of apps = 1\n1 : com.example.app\n (Allow incoming connections)\n"
        )
        .expect("bundle identity")[0]
            .subject,
        "com.example.app"
    );
    assert!(
        native::parse_alf_apps(
            "Total number of apps = 2\n1 : /usr/bin/test\n (Allow incoming connections)\n"
        )
        .is_err()
    );
}
// Existing output files and symlinks cannot be overwritten by a new collection.
#[test]
fn output_does_not_replace_an_existing_report() {
    let dir = tempfile::tempdir().expect("output directory");
    let path = dir.path().join("report");
    save(&path, b"first").expect("first report");
    assert!(save(&path, b"second").is_err());
    assert_eq!(std::fs::read(path).expect("retained report"), b"first");
}

// Accessible process evidence must survive an empty kernel table. A known live
// listener cannot be described as absent because another source returned no rows.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn native_socket_command_preserves_kernel_output() {
    let listener = std::net::TcpListener::bind("0.0.0.0:0").expect("test listener");
    let port = u32::from(listener.local_addr().expect("listener address").port());
    let report = collect().await.expect("native host");
    assert!(report.binds.iter().any(|v| {
        !v.owner.is_empty()
            && v.value
                .as_ref()
                .is_some_and(|v| v.protocol == "tcp" && v.port == port)
    }));
    assert!(report.coverage.iter().any(|v| v.module == "kernel-tcp"));
    assert!(!report.network.as_ref().expect("coverage").sockets_complete);
}

// The final serialization boundary must reject temporary literals even when a
// module accidentally emits one outside the interface-address path.
#[test]
fn temporary_bind_cannot_be_serialized() {
    let mut value = report();
    value.binds.push(api::HostBind {
        owner: "owner".into(),
        value: Some(api::SocketBind {
            bind: "2001:db8::123".into(),
            address_stability: api::AddressStability::Temporary as i32,
            ..Default::default()
        }),
    });
    assert!(encode(value).is_err());
}

// Equivalent IPv6 spelling and scoped/prefixed literals must not bypass the
// generation filter when the same temporary address appears in other sources.
#[test]
fn temporary_literals_are_removed_from_routes_and_binds() {
    let mut value = report();
    value.binds.push(api::HostBind {
        owner: String::new(),
        value: Some(api::SocketBind {
            bind: "2001:db8:0:0::1%en0".into(),
            ..Default::default()
        }),
    });
    value.network = Some(api::HostNetwork {
        routes: vec![api::LocalRoute {
            destination: "2001:db8::1/128".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    native::redact_literals(
        &mut value,
        &std::collections::BTreeSet::from(["2001:db8::1".into()]),
    );
    assert!(value.binds.is_empty());
    assert!(value.network.expect("network").routes.is_empty());
}

// This runs on the actual native host. Fixtures cannot substitute for working
// OS reads, coherent boot evidence or a file accepted by the common decoder.
#[tokio::test]
async fn native_host_collection_has_a_decodable_core() {
    let report = collect().await.expect("native collection");
    for coverage in &report.coverage {
        println!("native module {}: {}", coverage.module, coverage.status);
    }
    println!(
        "native findings: {}, binds: {}",
        report.findings.len(),
        report.binds.len()
    );
    assert_eq!(report.source_basis.len(), 64);
    assert_eq!(report.boot_basis.len(), 64);
    assert!(
        report
            .coverage
            .iter()
            .any(|v| v.module == "interfaces" && v.status == "verified")
    );
    for module in ["sockets", "routes"] {
        let coverage = report
            .coverage
            .iter()
            .find(|v| v.module == module)
            .expect("attempted native module");
        assert_eq!(coverage.status, "verified", "{module}: {}", coverage.detail);
    }
    assert!(
        report
            .findings
            .iter()
            .any(|v| v.kind == wire::AssetKind::Interface as i32)
    );
    let bytes = encode(report).expect("bounded native report");
    assert!(!bytes.is_empty());
}
