use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::vmess::{LatencyStatus, VmessNode};

/// Saved node data including latency measurements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedNode {
    pub v: String,
    pub ps: String,
    pub add: String,
    pub port: serde_json::Value,
    pub id: String,
    pub aid: serde_json::Value,
    pub net: String,
    #[serde(rename = "type")]
    pub type_field: String,
    pub host: String,
    pub path: String,
    pub tls: String,
    pub sni: String,
    pub alpn: String,
    pub fp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_latency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tcp_latency: Option<u64>,
    #[serde(default)]
    pub http_timed_out: bool,
    #[serde(default)]
    pub tcp_timed_out: bool,
}

impl From<&VmessNode> for SavedNode {
    fn from(node: &VmessNode) -> Self {
        let (http_latency, http_timed_out) = match node.http_latency {
            LatencyStatus::Success(ms) => (Some(ms), false),
            LatencyStatus::TimedOut => (None, true),
            LatencyStatus::NotTested => (None, false),
        };
        let (tcp_latency, tcp_timed_out) = match node.tcp_latency {
            LatencyStatus::Success(ms) => (Some(ms), false),
            LatencyStatus::TimedOut => (None, true),
            LatencyStatus::NotTested => (None, false),
        };
        SavedNode {
            v: node.v.clone(),
            ps: node.ps.clone(),
            add: node.add.clone(),
            port: node.port.clone(),
            id: node.id.clone(),
            aid: node.aid.clone(),
            net: node.net.clone(),
            type_field: node.type_field.clone(),
            host: node.host.clone(),
            path: node.path.clone(),
            tls: node.tls.clone(),
            sni: node.sni.clone(),
            alpn: node.alpn.clone(),
            fp: node.fp.clone(),
            http_latency,
            tcp_latency,
            http_timed_out,
            tcp_timed_out,
        }
    }
}

impl From<SavedNode> for VmessNode {
    fn from(saved: SavedNode) -> Self {
        let http_latency = if let Some(ms) = saved.http_latency {
            LatencyStatus::Success(ms)
        } else if saved.http_timed_out {
            LatencyStatus::TimedOut
        } else {
            LatencyStatus::NotTested
        };
        let tcp_latency = if let Some(ms) = saved.tcp_latency {
            LatencyStatus::Success(ms)
        } else if saved.tcp_timed_out {
            LatencyStatus::TimedOut
        } else {
            LatencyStatus::NotTested
        };
        VmessNode {
            v: saved.v,
            ps: saved.ps,
            add: saved.add,
            port: saved.port,
            id: saved.id,
            aid: saved.aid,
            net: saved.net,
            type_field: saved.type_field,
            host: saved.host,
            path: saved.path,
            tls: saved.tls,
            sni: saved.sni,
            alpn: saved.alpn,
            fp: saved.fp,
            http_latency,
            tcp_latency,
        }
    }
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribe_url: Option<String>,
    #[serde(default)]
    pub nodes: Vec<SavedNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_column: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
}

impl Config {
    /// Get the config file path (~/.config/subman.json)
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("subman.json"))
    }

    /// Load config from file, returns default if file doesn't exist
    pub fn load() -> Config {
        let Some(path) = Self::config_path() else {
            return Config::default();
        };

        if !path.exists() {
            return Config::default();
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::config_path() else {
            return Err(anyhow::anyhow!("Could not determine config directory"));
        };

        // Create config directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// Convert saved nodes to VmessNodes
    pub fn to_vmess_nodes(&self) -> Vec<VmessNode> {
        self.nodes.iter().cloned().map(VmessNode::from).collect()
    }

    /// Update nodes from VmessNodes
    pub fn update_nodes(&mut self, nodes: &[VmessNode]) {
        self.nodes = nodes.iter().map(SavedNode::from).collect();
    }
}
