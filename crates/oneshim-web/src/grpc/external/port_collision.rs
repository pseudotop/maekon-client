//! Collision check between external and loopback gRPC ports.
//!
//! Both servers bind on the same interface (default 127.0.0.1 for the
//! loopback, configurable external address for external gRPC); if the
//! ports collide, the second `TcpListener::bind` returns `AddrInUse` and
//! the server fails to start. The launcher calls this pre-bind to fail
//! fast with a human-readable error instead of a cryptic OS-level one.

/// Return an error if `external_port` equals `loopback_port`. Otherwise Ok.
pub fn check_port_collision(external_port: u16, loopback_port: u16) -> Result<(), String> {
    if external_port == loopback_port {
        Err(format!(
            "external_grpc.port ({external_port}) collides with loopback gRPC port ({loopback_port})"
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_same_port_errors() {
        let e = check_port_collision(10091, 10091).unwrap_err();
        assert!(
            e.contains("10091"),
            "error must mention the port number; got: {e}"
        );
    }

    #[test]
    fn distinct_ports_ok() {
        assert!(check_port_collision(10092, 10091).is_ok());
    }
}
