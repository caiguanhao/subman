use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};

/// Latency test result status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LatencyStatus {
    /// Not tested yet
    #[default]
    NotTested,
    /// Test timed out
    TimedOut,
    /// Test succeeded with latency in ms
    Success(u64),
}

impl LatencyStatus {
    pub fn is_tested(&self) -> bool {
        !matches!(self, LatencyStatus::NotTested)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmessNode {
    #[serde(default)]
    pub v: String,
    #[serde(default)]
    pub ps: String,
    #[serde(default)]
    pub add: String,
    #[serde(default)]
    pub port: serde_json::Value,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub aid: serde_json::Value,
    #[serde(default)]
    pub net: String,
    #[serde(default, rename = "type")]
    pub type_field: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub tls: String,
    #[serde(default)]
    pub sni: String,
    #[serde(default)]
    pub alpn: String,
    #[serde(default)]
    pub fp: String,
    // Runtime fields for latency
    #[serde(skip)]
    pub http_latency: LatencyStatus,
    #[serde(skip)]
    pub tcp_latency: LatencyStatus,
}

impl Default for VmessNode {
    fn default() -> Self {
        Self {
            v: String::new(),
            ps: String::new(),
            add: String::new(),
            port: serde_json::Value::Null,
            id: String::new(),
            aid: serde_json::Value::Null,
            net: String::new(),
            type_field: String::new(),
            host: String::new(),
            path: String::new(),
            tls: String::new(),
            sni: String::new(),
            alpn: String::new(),
            fp: String::new(),
            http_latency: LatencyStatus::NotTested,
            tcp_latency: LatencyStatus::NotTested,
        }
    }
}

impl VmessNode {
    /// Parse a vmess:// link into a VmessNode
    pub fn from_link(link: &str) -> Result<Self> {
        let link = link.trim();
        if !link.starts_with("vmess://") {
            return Err(anyhow!("Invalid vmess link: must start with vmess://"));
        }

        let encoded = &link[8..];
        // Try standard base64 first, then URL-safe
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .or_else(|_| general_purpose::URL_SAFE.decode(encoded))
            .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded))
            .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(encoded))
            .map_err(|e| anyhow!("Base64 decode error: {e}"))?;

        let json_str =
            String::from_utf8(decoded).map_err(|e| anyhow!("UTF-8 decode error: {e}"))?;

        let node: VmessNode =
            serde_json::from_str(&json_str).map_err(|e| anyhow!("JSON parse error: {e}"))?;

        Ok(node)
    }

    /// Get the port as u16
    pub fn get_port(&self) -> u16 {
        match &self.port {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(443) as u16,
            serde_json::Value::String(s) => s.parse().unwrap_or(443),
            _ => 443,
        }
    }

    /// Get the alterId as u32
    pub fn get_aid(&self) -> u32 {
        match &self.aid {
            serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u32,
            serde_json::Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// Get display name (ps field or address:port if ps is empty)
    pub fn display_name(&self) -> String {
        if self.ps.is_empty() {
            format!("{}:{}", self.add, self.get_port())
        } else {
            self.ps.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vmess_link() {
        // Example vmess link (base64 encoded JSON)
        let json = r#"{"v":"2","ps":"Test Node","add":"example.com","port":"443","id":"uuid-here","aid":"0","net":"ws","type":"none","host":"","path":"/path","tls":"tls"}"#;
        let encoded = general_purpose::STANDARD.encode(json);
        let link = format!("vmess://{}", encoded);

        let node = VmessNode::from_link(&link).unwrap();
        assert_eq!(node.ps, "Test Node");
        assert_eq!(node.add, "example.com");
        assert_eq!(node.get_port(), 443);
    }
}
