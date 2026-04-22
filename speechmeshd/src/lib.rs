extern crate self as speechmeshd;

pub mod agent;
#[path = "bin/speechmesh-agent.rs"]
pub mod device_agent_app;
pub mod providers;
pub mod server;

/// 从 speechmesh-bridges crate 重新导出，保持向后兼容
pub mod asr_bridge {
    pub use speechmesh_bridges::BridgeError;
    pub use speechmesh_bridges::asr::*;
}

pub mod tts_bridge {
    pub use speechmesh_bridges::tts::*;
}

pub mod bridge_support {
    pub use speechmesh_bridges::BridgeError;
}

pub mod tcp_keepalive {
    use std::time::Duration;

    /// Configure TCP keepalive on a tokio TcpStream.
    ///
    /// This prevents NAT/firewall/VPN middleboxes from killing idle
    /// connections by sending periodic TCP keepalive probes at the OS
    /// level.
    pub fn configure(stream: &tokio::net::TcpStream, interval: Duration) -> std::io::Result<()> {
        let socket = socket2::SockRef::from(stream);
        let keepalive = socket2::TcpKeepalive::new()
            .with_time(interval)
            .with_interval(interval);
        socket.set_tcp_keepalive(&keepalive)?;
        Ok(())
    }

    /// Default keepalive interval: 15 seconds.
    ///
    /// Aggressive enough to survive most NAT/VPN idle timeouts (which
    /// are typically 30-120 s) while adding negligible overhead.
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(15);
}
