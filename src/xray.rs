use anyhow::{anyhow, Result};
use serde_json::json;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

use crate::vmess::VmessNode;

/// Default xray config path
pub const DEFAULT_XRAY_CONFIG_PATH: &str = "/opt/homebrew/etc/xray/config.json";
/// Default SOCKS port
pub const DEFAULT_SOCKS_PORT: u16 = 1080;

/// Active node info extracted from xray config
#[derive(Debug, Clone)]
pub struct ActiveNodeInfo {
    pub address: String,
    pub port: u16,
    pub user_id: String,
}

/// Read the current xray config and extract the active node info
pub fn read_active_node(config_path: &str) -> Option<ActiveNodeInfo> {
    let content = fs::read_to_string(config_path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&content).ok()?;

    // Navigate to outbounds[0].settings.vnext[0]
    let vnext = config
        .get("outbounds")?
        .get(0)?
        .get("settings")?
        .get("vnext")?
        .get(0)?;

    let address = vnext.get("address")?.as_str()?.to_string();
    let port = vnext.get("port")?.as_u64()? as u16;
    let user_id = vnext
        .get("users")?
        .get(0)?
        .get("id")?
        .as_str()?
        .to_string();

    Some(ActiveNodeInfo {
        address,
        port,
        user_id,
    })
}

/// Find the index of the active node in the nodes list
pub fn find_active_node_index(nodes: &[VmessNode], active: &ActiveNodeInfo) -> Option<usize> {
    nodes.iter().position(|node| {
        node.add == active.address && node.get_port() == active.port && node.id == active.user_id
    })
}

/// Generate xray config JSON for a vmess node
pub fn generate_config(node: &VmessNode, socks_port: u16) -> serde_json::Value {
    let port = node.get_port();
    let aid = node.get_aid();

    // Build stream settings based on network type
    let mut stream_settings = json!({
        "network": if node.net.is_empty() { "tcp" } else { &node.net }
    });

    // Add WebSocket settings if network is ws
    if node.net == "ws" {
        let mut ws_settings = json!({});
        if !node.path.is_empty() {
            ws_settings["path"] = json!(node.path);
        }
        if !node.host.is_empty() {
            ws_settings["headers"] = json!({
                "Host": node.host
            });
        }
        stream_settings["wsSettings"] = ws_settings;
    }

    // Add TCP settings if type is http
    if node.net == "tcp" && node.type_field == "http" {
        stream_settings["tcpSettings"] = json!({
            "header": {
                "type": "http",
                "request": {
                    "path": [if node.path.is_empty() { "/" } else { &node.path }],
                    "headers": {
                        "Host": [if node.host.is_empty() { &node.add } else { &node.host }]
                    }
                }
            }
        });
    }

    // Add TLS settings
    if node.tls == "tls" {
        stream_settings["security"] = json!("tls");
        let mut tls_settings = json!({});
        if !node.sni.is_empty() {
            tls_settings["serverName"] = json!(node.sni);
        } else if !node.host.is_empty() {
            tls_settings["serverName"] = json!(node.host);
        }
        if !node.alpn.is_empty() {
            tls_settings["alpn"] = json!(node.alpn.split(',').collect::<Vec<_>>());
        }
        if !node.fp.is_empty() {
            tls_settings["fingerprint"] = json!(node.fp);
        }
        stream_settings["tlsSettings"] = tls_settings;
    }

    // Build the full config
    json!({
        "log": {
            "loglevel": "warning"
        },
        "inbounds": [
            {
                "port": socks_port,
                "listen": "127.0.0.1",
                "protocol": "socks",
                "settings": {
                    "udp": true
                }
            }
        ],
        "outbounds": [
            {
                "protocol": "vmess",
                "settings": {
                    "vnext": [
                        {
                            "address": node.add,
                            "port": port,
                            "users": [
                                {
                                    "id": node.id,
                                    "alterId": aid,
                                    "security": "auto"
                                }
                            ]
                        }
                    ]
                },
                "streamSettings": stream_settings
            }
        ]
    })
}

/// Save xray config to the specified path
pub fn save_config_with_path(node: &VmessNode, config_path: &str) -> Result<()> {
    let config = generate_config(node, DEFAULT_SOCKS_PORT);
    let config_str = serde_json::to_string_pretty(&config)?;

    std::fs::write(config_path, config_str)
        .map_err(|e| anyhow!("Failed to write config to {config_path}: {e}"))?;

    Ok(())
}

/// Save xray config to a custom path (for latency testing)
pub fn save_config_to_path(node: &VmessNode, path: &str, socks_port: u16) -> Result<()> {
    let config = generate_config(node, socks_port);
    let config_str = serde_json::to_string_pretty(&config)?;

    std::fs::write(path, config_str)
        .map_err(|e| anyhow!("Failed to write config to {path}: {e}"))?;

    Ok(())
}

/// Get xray process ID
fn get_xray_pid() -> Option<u32> {
    let output = Command::new("pgrep").arg("xray").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Get the first PID if there are multiple
    stdout.lines().next()?.trim().parse().ok()
}

/// Restart result containing PIDs
pub struct RestartResult {
    pub old_pid: u32,
    pub new_pid: u32,
}

/// Restart xray service by sending SIGHUP signal
pub fn restart_xray_service() -> Result<RestartResult> {
    // Get current PID
    let old_pid = get_xray_pid().ok_or_else(|| anyhow!("xray process not found"))?;

    // Send SIGHUP to reload config
    let output = Command::new("kill")
        .args(["-HUP", &old_pid.to_string()])
        .output()
        .map_err(|e| anyhow!("Failed to send HUP signal: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Failed to send HUP to xray (PID {old_pid}): {stderr}"
        ));
    }

    // Poll every 500ms for up to 3 seconds to check if PID changed (restart success)
    let max_attempts = 6; // 6 * 500ms = 3000ms
    for attempt in 0..max_attempts {
        thread::sleep(Duration::from_millis(500));

        match get_xray_pid() {
            Some(new_pid) => {
                if new_pid != old_pid {
                    // PID changed - restart success
                    return Ok(RestartResult { old_pid, new_pid });
                }
                // PID unchanged, keep waiting unless this is the last attempt
                if attempt == max_attempts - 1 {
                    return Err(anyhow!(
                        "xray PID unchanged ({old_pid}) after 3 seconds - restart failed"
                    ));
                }
            }
            None => {
                // Process not found, wait a bit more unless this is the last attempt
                if attempt == max_attempts - 1 {
                    return Err(anyhow!(
                        "xray process (PID {old_pid}) disappeared after HUP signal"
                    ));
                }
            }
        }
    }

    Err(anyhow!(
        "Timeout waiting for xray (PID {old_pid}) to restart"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::LatencyStatus;

    #[test]
    fn test_generate_config() {
        let node = VmessNode {
            v: "2".to_string(),
            ps: "Test".to_string(),
            add: "test.com".to_string(),
            port: serde_json::json!(443),
            id: "test-uuid".to_string(),
            aid: serde_json::json!(0),
            net: "ws".to_string(),
            type_field: "none".to_string(),
            host: "cdn.test.com".to_string(),
            path: "/ws".to_string(),
            tls: "tls".to_string(),
            sni: "".to_string(),
            alpn: "".to_string(),
            fp: "".to_string(),
            http_latency: LatencyStatus::NotTested,
            tcp_latency: LatencyStatus::NotTested,
        };

        let config = generate_config(&node, 1080);
        assert!(config["inbounds"][0]["port"] == 1080);
        assert!(config["outbounds"][0]["protocol"] == "vmess");
    }
}
