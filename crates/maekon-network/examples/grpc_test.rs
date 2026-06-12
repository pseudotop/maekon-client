//!   cargo run -p maekon-network --example grpc_test --features grpc
//!   NO_EMOJI=1 cargo run -p maekon-network --example grpc_test --features grpc
#![allow(deprecated)] // Example uses simple constructors for brevity

use std::collections::HashMap;
use std::sync::Arc;

use maekon_network::auth::TokenManager;
use maekon_network::grpc::{FeedbackAction, GrpcConfig, UnifiedClient, UploadBatchRequest};

struct Output {
    use_emoji: bool,
}

impl Output {
    fn new() -> Self {
        let use_emoji = std::env::var("NO_EMOJI").is_err();
        Self { use_emoji }
    }

    fn ok(&self) -> &'static str {
        if self.use_emoji {
            "✅"
        } else {
            "[OK]"
        }
    }

    fn err(&self) -> &'static str {
        if self.use_emoji {
            "❌"
        } else {
            "[ERR]"
        }
    }

    fn info(&self) -> &'static str {
        if self.use_emoji {
            "ℹ️"
        } else {
            "[INFO]"
        }
    }

    fn warn(&self) -> &'static str {
        if self.use_emoji {
            "⚠️"
        } else {
            "[WARN]"
        }
    }

    fn timeout(&self) -> &'static str {
        if self.use_emoji {
            "⏱️"
        } else {
            "[TIMEOUT]"
        }
    }

    fn msg(&self) -> &'static str {
        if self.use_emoji {
            "📨"
        } else {
            "[MSG]"
        }
    }
}

fn fixture_password() -> String {
    std::env::var("MAEKON_GRPC_TEST_PASSWORD").unwrap_or_else(|_| {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("grpc-test-{nonce:x}")
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let out = Output::new();

    println!("============================================================");
    println!("Rust gRPC client test");
    println!("server: localhost:50052");
    if !out.use_emoji {
        println!("mode: ASCII (NO_EMOJI=1)");
    }
    println!("============================================================");

    let grpc_config = GrpcConfig {
        use_grpc_auth: true,
        use_grpc_context: true,
        grpc_endpoint: "http://127.0.0.1:50052".to_string(),
        grpc_fallback_ports: vec![50051, 50053], // fallback port list
        rest_endpoint: "http://127.0.0.1:8000".to_string(),
        connect_timeout_secs: 10,
        request_timeout_secs: 30,
        use_tls: false,
        mtls_enabled: false,
        tls_domain_name: None,
        tls_ca_cert_path: None,
        tls_client_cert_path: None,
        tls_client_key_path: None,
        rest_tls: None,
    };

    let token_manager = Arc::new(TokenManager::new("http://127.0.0.1:8000"));

    let client = UnifiedClient::new(grpc_config, token_manager)?;

    println!("\n=== 1. login test ===");
    let password = fixture_password();
    match client
        .login("admin@example.com", &password, "test-org-001")
        .await
    {
        Ok(response) => {
            println!("  {} login success", out.ok());
            println!("  user_id present: {}", response.user_id.is_some());
            println!(
                "  access_token present: {}",
                !response.access_token.is_empty()
            );
        }
        Err(e) => {
            println!("  {} login failure: {}", out.err(), e);
        }
    }

    println!("\n=== 2. session create test ===");
    let device_info: HashMap<String, String> = [
        ("os".to_string(), "macOS".to_string()),
        ("version".to_string(), "0.1.3".to_string()),
    ]
    .into_iter()
    .collect();

    match client.create_session("rust-client-001", device_info).await {
        Ok(response) => {
            println!("  {} session create success", out.ok());
            println!("  session_id present: {}", !response.session_id.is_empty());
            println!("  user_id present: {}", !response.user_id.is_empty());

            println!("\n=== 3. heartbeat test ===");
            match client.heartbeat(&response.session_id).await {
                Ok(success) => {
                    println!("  {} heartbeat success: {}", out.ok(), success);
                }
                Err(e) => {
                    println!("  {} heartbeat failure: {}", out.err(), e);
                }
            }

            println!("\n=== 3.5. batch upload test ===");
            let batch_request = UploadBatchRequest {
                session_id: response.session_id.clone(),
                events: vec![], // event upload test
                frames: vec![], // frame upload test
            };
            match client.upload_batch(batch_request).await {
                Ok(batch_response) => {
                    println!("  {} batch upload success", out.ok());
                    println!("    accepted_count: {}", batch_response.accepted_count);
                }
                Err(e) => {
                    println!("  {} batch upload failure: {}", out.err(), e);
                }
            }

            println!("\n=== 4. suggestion test ===");
            #[allow(deprecated)] // intentional: example exercises the test-only no-token path
            match client.subscribe_suggestions(&response.session_id).await {
                Ok(mut stream) => {
                    println!("{} suggestion stream subscribe success", out.ok());
                    println!("first suggestion waiting in progress... (5s timeout)");

                    let timeout_duration = std::time::Duration::from_secs(5);
                    match tokio::time::timeout(timeout_duration, stream.message()).await {
                        Ok(Ok(Some(suggestion))) => {
                            println!("  {} suggestion received:", out.msg());
                            println!("    suggestion_id: {}", suggestion.suggestion_id);
                            println!("    content: {}", suggestion.content);
                            println!("    priority: {:?}", suggestion.priority);
                        }
                        Ok(Ok(None)) => {
                            println!("{} stream stopped (server ended)", out.info());
                        }
                        Ok(Err(e)) => {
                            println!("{} stream error: {}", out.warn(), e);
                        }
                        Err(_) => {
                            println!(
                                "{} timeout (5s within suggestion none - normal)",
                                out.timeout()
                            );
                        }
                    }
                }
                Err(e) => {
                    println!("{} suggestion stream subscribe failure: {}", out.err(), e);
                }
            }

            println!("\n=== 5. feedback sent test ===");
            match client
                .send_feedback(
                    "test-suggestion-001",
                    FeedbackAction::Accepted,
                    Some("test feedback"),
                )
                .await
            {
                Ok(()) => {
                    println!("  {} feedback sent success", out.ok());
                }
                Err(e) => {
                    println!("  {} feedback sent failure: {}", out.err(), e);
                }
            }
        }
        Err(e) => {
            println!("  {} session create failure: {}", out.err(), e);
        }
    }

    println!("\n============================================================");
    println!("test completed");
    println!("============================================================");

    Ok(())
}
