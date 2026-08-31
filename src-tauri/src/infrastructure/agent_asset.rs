use sha2::{Digest, Sha256};

use crate::core::error::{AppError, AppResult};

pub const AGENT_SOURCE: &str = include_str!("../../resources/agent/edge-node-agent.sh");
pub const AGENT_VERSION: &str = "0.1.2";
pub const AGENT_PROTOCOL_VERSION: &str = "1";
pub const AGENT_SHA256: &str = "b49eb802e3c31a1dfdc48e31630820d057e8f6d38a2e4dadff314697a3e0b678";
pub const AGENT_COMPATIBILITY: &[&str] = &[
    "Linux x86_64",
    "Docker Engine 20.10+",
    "docker-compose v1 或 docker compose v2",
    "POSIX sh、tar、awk",
];

pub fn verify_embedded_agent() -> AppResult<()> {
    let actual = hex::encode(Sha256::digest(AGENT_SOURCE.as_bytes()));
    if actual != AGENT_SHA256 {
        return Err(AppError::Integrity {
            operation: "校验内嵌Agent哈希",
        });
    }
    let version = format!("AGENT_VERSION=\"{AGENT_VERSION}\"");
    let protocol = format!("AGENT_PROTOCOL_VERSION=\"{AGENT_PROTOCOL_VERSION}\"");
    if !AGENT_SOURCE.lines().any(|line| line == version)
        || !AGENT_SOURCE.lines().any(|line| line == protocol)
    {
        return Err(AppError::Integrity {
            operation: "校验内嵌Agent版本契约",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_COMPATIBILITY, AGENT_PROTOCOL_VERSION, AGENT_SHA256, AGENT_SOURCE, AGENT_VERSION,
        verify_embedded_agent,
    };

    #[test]
    fn embedded_agent_has_version_hash_protocol_and_compatibility() {
        verify_embedded_agent().expect("embedded agent");
        assert_eq!(AGENT_VERSION, "0.1.2");
        assert_eq!(AGENT_PROTOCOL_VERSION, "1");
        assert_eq!(AGENT_SHA256.len(), 64);
        assert!(!AGENT_COMPATIBILITY.is_empty());
        assert!(AGENT_SOURCE.contains("require_safe_release_version"));
        assert!(AGENT_SOURCE.contains("REMOTE_COMPOSE"));
    }
}
