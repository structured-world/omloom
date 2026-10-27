use super::*;

// Both HTTP/1 Host and HTTP/2 authority must identify this listener, including when both exist.
#[test]
fn fixture_requests_require_a_consistent_loopback_authority() {
    use axum::{body::Body, http::Request};
    for value in ["127.0.0.1:9086", "localhost:9086", "LOCALHOST:9086"] {
        let request = Request::builder()
            .uri("/")
            .header("host", value)
            .body(Body::empty())
            .expect("request");
        assert!(loopback_request(&request, 9086));
        let request = Request::builder()
            .uri(format!("http://{value}/"))
            .body(Body::empty())
            .expect("request");
        assert!(loopback_request(&request, 9086));
    }
    for value in [
        "rebind.example:9086",
        "127.0.0.1:80",
        "localhost",
        "user@localhost:9086",
        "localhost:9086,evil.example:9086",
    ] {
        assert!(!loopback_authority(value, 9086));
        let request = Request::builder()
            .uri("/")
            .header("host", value)
            .body(Body::empty())
            .expect("request");
        assert!(!loopback_request(&request, 9086));
    }
    let request = Request::builder()
        .uri("http://rebind.example:9086/")
        .header("host", "localhost:9086")
        .body(Body::empty())
        .expect("request");
    assert!(!loopback_request(&request, 9086));
    let request = Request::builder()
        .uri("http://localhost:9086/")
        .header("host", "rebind.example:9086")
        .body(Body::empty())
        .expect("request");
    assert!(!loopback_request(&request, 9086));
    let request = Request::builder()
        .uri("/")
        .header("host", "localhost:9086")
        .header("host", "localhost:9086")
        .body(Body::empty())
        .expect("request");
    assert!(!loopback_request(&request, 9086));
    assert!(!loopback_request(&Request::new(Body::empty()), 9086));
}
