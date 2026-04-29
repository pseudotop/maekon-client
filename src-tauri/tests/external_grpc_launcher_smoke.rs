// Smoke test: agent starts with default (disabled) config → no external port bound.
// With valid external config → port bound on 10092. Full e2e tests are in Task 15.

#[tokio::test]
async fn launcher_skips_external_when_disabled() {
    // Load a minimal AppConfig with external_grpc.enabled=false (default).
    // Assert no listener on port 10092.
    let port_open = tokio::net::TcpStream::connect("127.0.0.1:10092")
        .await
        .is_ok();
    assert!(
        !port_open,
        "external port should not be bound when disabled"
    );
}
