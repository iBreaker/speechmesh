pub mod agent;
pub mod providers;
pub mod server;

/// 从 speechmesh-bridges crate 重新导出，保持向后兼容
pub mod asr_bridge {
    pub use speechmesh_bridges::asr::*;
    pub use speechmesh_bridges::BridgeError;
}

pub mod tts_bridge {
    pub use speechmesh_bridges::tts::*;
}

pub mod bridge_support {
    pub use speechmesh_bridges::BridgeError;
}
