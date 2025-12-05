use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::vmess::{LatencyStatus, VmessNode};
use crate::xray::save_config_to_path;

const TEST_URL: &str = "https://www.google.com/generate_204";
const TEST_TIMEOUT_SECS: u64 = 5;

// Atomic counter for allocating unique ports
static PORT_COUNTER: AtomicU16 = AtomicU16::new(10800);

/// Test type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestType {
    Http,
    Tcp,
}

/// Get a unique port for testing (wraps around if exceeds 60000)
fn get_test_port() -> u16 {
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
    if port > 60000 {
        PORT_COUNTER.store(10800, Ordering::SeqCst);
        return 10800;
    }
    port
}

/// Reset the port counter (call before batch testing)
pub fn reset_port_counter() {
    PORT_COUNTER.store(10800, Ordering::SeqCst);
}

/// RAII guard for xray process and config file
struct ProcessGuard {
    #[allow(dead_code)]
    child: Child,
    config_path: String,
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        // Process is killed by kill_on_drop(true) in Child
        // We just need to remove the config file
        let _ = std::fs::remove_file(&self.config_path);
    }
}

/// Wait for port to be open
async fn wait_for_port(port: u16) -> bool {
    for _ in 0..20 {
        // 2 seconds max (20 * 100ms)
        if TcpStream::connect(format!("127.0.0.1:{port}")).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Start xray process with the given config
async fn start_xray(config_path: &str, port: u16) -> Result<ProcessGuard> {
    let child = Command::new("xray")
        .args(["run", "-c", config_path])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;

    if !wait_for_port(port).await {
        return Err(anyhow::anyhow!("Xray failed to start on port {port}"));
    }

    Ok(ProcessGuard {
        child,
        config_path: config_path.to_string(),
    })
}

/// Test HTTP latency for a single node (via xray proxy)
pub async fn test_node_http_latency(node: &VmessNode) -> LatencyStatus {
    let port = get_test_port();
    let config_path = format!("/tmp/xray_test_{port}.json");

    // Generate and save config
    if save_config_to_path(node, &config_path, port).is_err() {
        return LatencyStatus::TimedOut;
    }

    // Start xray
    let _guard = match start_xray(&config_path, port).await {
        Ok(g) => g,
        Err(_) => {
            let _ = std::fs::remove_file(&config_path);
            return LatencyStatus::TimedOut;
        }
    };

    // Create HTTP client with SOCKS5 proxy
    let proxy = match reqwest::Proxy::all(format!("socks5://127.0.0.1:{port}")) {
        Ok(p) => p,
        Err(_) => return LatencyStatus::TimedOut,
    };

    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(TEST_TIMEOUT_SECS))
        .build()
    {
        Ok(c) => c,
        Err(_) => return LatencyStatus::TimedOut,
    };

    // Measure latency
    let start = Instant::now();
    let result = client.get(TEST_URL).send().await;
    let latency = start.elapsed().as_millis() as u64;

    // Guard drops here, killing process and removing file

    match result {
        Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 204 => {
            LatencyStatus::Success(latency)
        }
        _ => LatencyStatus::TimedOut,
    }
}

/// Test TCP connection latency for a single node (direct connection to node's address)
pub async fn test_node_tcp_latency(node: &VmessNode) -> LatencyStatus {
    let addr = format!("{}:{}", node.add, node.get_port());

    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(TEST_TIMEOUT_SECS),
        TcpStream::connect(&addr),
    )
    .await;

    match result {
        Ok(Ok(_stream)) => LatencyStatus::Success(start.elapsed().as_millis() as u64),
        _ => LatencyStatus::TimedOut,
    }
}

/// Message for latency test results
pub struct LatencyResult {
    pub index: usize,
    pub latency: LatencyStatus,
    pub test_type: TestType,
}

/// Test latency for all nodes in parallel
/// Returns the cancel flag that can be used to stop the test
pub async fn test_all_latencies(
    nodes: Vec<VmessNode>,
    result_tx: mpsc::Sender<LatencyResult>,
    max_concurrent: usize,
    test_type: TestType,
    cancel_flag: Arc<AtomicBool>,
) {
    reset_port_counter();

    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let mut handles = Vec::new();

    for (index, node) in nodes.into_iter().enumerate() {
        // Check if cancelled before starting new test
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let tx = result_tx.clone();
        let cancel = cancel_flag.clone();

        let handle = tokio::spawn(async move {
            // Check if cancelled
            if cancel.load(Ordering::SeqCst) {
                drop(permit);
                return;
            }

            let latency = match test_type {
                TestType::Http => test_node_http_latency(&node).await,
                TestType::Tcp => test_node_tcp_latency(&node).await,
            };

            // Only send result if not cancelled
            if !cancel.load(Ordering::SeqCst) {
                let _ = tx
                    .send(LatencyResult {
                        index,
                        latency,
                        test_type,
                    })
                    .await;
            }
            drop(permit);
        });

        handles.push(handle);
    }

    // Wait for all tests to complete
    for handle in handles {
        let _ = handle.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_counter() {
        reset_port_counter();
        assert_eq!(get_test_port(), 10800);
        assert_eq!(get_test_port(), 10801);
        assert_eq!(get_test_port(), 10802);
        reset_port_counter();
        assert_eq!(get_test_port(), 10800);
    }
}
