use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine};

use crate::vmess::VmessNode;

/// Fetch subscription content from URL and parse into vmess nodes
pub async fn fetch_subscription(url: &str) -> Result<Vec<VmessNode>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = client
        .get(url)
        .header("User-Agent", "subman/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch subscription: {e}"))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "HTTP error: {} {}",
            response.status().as_u16(),
            response.status().canonical_reason().unwrap_or("Unknown")
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow!("Failed to read response body: {e}"))?;

    parse_subscription_content(&body)
}

/// Parse base64-encoded subscription content into vmess nodes
pub fn parse_subscription_content(content: &str) -> Result<Vec<VmessNode>> {
    let content = content.trim();

    // Try to decode as base64
    let decoded = general_purpose::STANDARD
        .decode(content)
        .or_else(|_| general_purpose::URL_SAFE.decode(content))
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(content))
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(content))
        .map_err(|e| anyhow!("Base64 decode error: {e}"))?;

    let decoded_str =
        String::from_utf8(decoded).map_err(|e| anyhow!("UTF-8 decode error: {e}"))?;

    // Parse each line as a vmess link
    let mut nodes = Vec::new();
    for line in decoded_str.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Only parse vmess:// links
        if line.starts_with("vmess://") {
            match VmessNode::from_link(line) {
                Ok(node) => nodes.push(node),
                Err(e) => {
                    // Log but don't fail on individual parse errors
                    eprintln!("Warning: Failed to parse vmess link: {e}");
                }
            }
        }
    }

    if nodes.is_empty() {
        return Err(anyhow!("No valid vmess nodes found in subscription"));
    }

    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_subscription_content() {
        use base64::engine::general_purpose;

        // Create a test subscription with two nodes
        let node1_json = r#"{"ps":"Node1","add":"n1.test.com","port":443,"id":"uuid1","aid":0,"net":"tcp","type":"none","host":"","path":"","tls":"tls"}"#;
        let node2_json = r#"{"ps":"Node2","add":"n2.test.com","port":8443,"id":"uuid2","aid":0,"net":"ws","type":"none","host":"","path":"/ws","tls":"tls"}"#;

        let link1 = format!("vmess://{}", general_purpose::STANDARD.encode(node1_json));
        let link2 = format!("vmess://{}", general_purpose::STANDARD.encode(node2_json));

        let content = format!("{}\n{}", link1, link2);
        let encoded = general_purpose::STANDARD.encode(&content);

        let nodes = parse_subscription_content(&encoded).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].ps, "Node1");
        assert_eq!(nodes[1].ps, "Node2");
    }
}
