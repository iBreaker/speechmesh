use thiserror::Error;

#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("bridge unavailable: {0}")]
    Unavailable(String),
    #[error("bridge disconnected: {0}")]
    Disconnected(String),
    #[error("bridge protocol error: {0}")]
    Protocol(String),
    #[error("bridge io error: {0}")]
    Io(String),
}
