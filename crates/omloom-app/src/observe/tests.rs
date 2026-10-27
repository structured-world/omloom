use super::*;
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::get,
};

// These are protocol fixtures; the independent live provider qualification is separate.
#[tokio::test]
async fn token_probe_keeps_interface_addresses_and_omits_local_addresses() {
    let app=Router::new()
        .route("/api2/json/version",get(|headers:HeaderMap|async move {
            if headers.get(AUTHORIZATION).and_then(|h|h.to_str().ok())!=Some("PVEAPIToken=reader@pve!observe=test-secret") { return (StatusCode::UNAUTHORIZED,Json(serde_json::json!({"data":null}))); }
            (StatusCode::OK,Json(serde_json::json!({"data":{"version":"9.2.21"}})))
        }))
        .route("/api2/json/nodes",get(||async{Json(serde_json::json!({"data":[{"node":"test-node"}]}))}))
        .route("/api2/json/nodes/test-node/status",get(||async{Json(serde_json::json!({"data":{"cpuinfo":{"model":"Test CPU"},"memory":{"total":17179869184u64}}}))}))
        .route("/api2/json/nodes/test-node/network",get(||async{Json(serde_json::json!({"data":[{"iface":"vmbr0","cidr":"192.0.2.10/24","cidr6":"2001:db8::10/64","vlan-id":42},{"iface":"lo","cidr":"127.0.0.1/8"}]}))}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let result = probe(api::ObserveProxmox {
        endpoint: format!("http://{address}"),
        token_id: "reader@pve!observe".into(),
        token_secret: "test-secret".into(),
        ..Default::default()
    })
    .await;
    server.abort();
    assert!(server.await.expect_err("cancelled").is_cancelled());
    let result = result.expect("probe");
    assert_eq!(result.nodes.len(), 1);
    assert_eq!(result.nodes[0].addresses.len(), 2);
    assert!(
        result.nodes[0]
            .addresses
            .iter()
            .all(|a| a.interface_name.as_deref() == Some("vmbr0") && a.vlan_tag == Some(42))
    );
    assert_eq!(result.nodes[0].memory_bytes, Some(17179869184));
    assert!(!format!("{result:?}").contains("test-secret"));
}

// Mixed credentials and URL userinfo are rejected without sending any request or echoing secrets.
#[tokio::test]
async fn probe_rejects_ambiguous_credentials_and_secret_urls() {
    for request in [
        api::ObserveProxmox {
            endpoint: "https://user:secret@example.invalid/".into(),
            ..Default::default()
        },
        api::ObserveProxmox {
            endpoint: "https://example.invalid/".into(),
            username: "user".into(),
            password: "secret".into(),
            token_id: "other".into(),
            token_secret: "other-secret".into(),
        },
    ] {
        let failure = probe(request).await.expect_err("reject before I/O");
        assert_eq!(failure.code(), Code::InvalidArgument);
        assert!(!failure.message().contains("secret"));
    }
}

// The seventeenth item is rejected by the seed before decoding its fields or growing the list.
#[test]
fn node_array_has_a_preallocation_bound() {
    let json=serde_json::json!({"data":(0..17).map(|_|serde_json::json!({"node":"n"})).collect::<Vec<_>>()}).to_string();
    assert!(serde_json::from_str::<Envelope<Bounded<Node, 16>>>(&json).is_err());
}
