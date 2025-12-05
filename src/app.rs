use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::Config;
use crate::latency::{LatencyResult, TestType};
use crate::vmess::{LatencyStatus, VmessNode};
use crate::xray::{find_active_node_index, read_active_node};

/// Sort column options
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    None,
    Name,
    Tcp,
    Http,
}

impl SortColumn {
    /// Cycle to next sort column
    pub fn next(self) -> Self {
        match self {
            SortColumn::None => SortColumn::Tcp,
            SortColumn::Tcp => SortColumn::Http,
            SortColumn::Http => SortColumn::Name,
            SortColumn::Name => SortColumn::None,
        }
    }

    /// Convert to string for serialization
    pub fn to_str(self) -> Option<&'static str> {
        match self {
            SortColumn::None => None,
            SortColumn::Name => Some("name"),
            SortColumn::Tcp => Some("tcp"),
            SortColumn::Http => Some("http"),
        }
    }

    /// Parse from string
    pub fn from_str(s: Option<&str>) -> Self {
        match s {
            Some("name") => SortColumn::Name,
            Some("tcp") => SortColumn::Tcp,
            Some("http") => SortColumn::Http,
            _ => SortColumn::None,
        }
    }
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn toggle(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }

    /// Convert to string for serialization
    pub fn to_str(self) -> &'static str {
        match self {
            SortDirection::Ascending => "asc",
            SortDirection::Descending => "desc",
        }
    }

    /// Parse from string
    pub fn from_str(s: Option<&str>) -> Self {
        match s {
            Some("desc") => SortDirection::Descending,
            _ => SortDirection::Ascending,
        }
    }
}

/// Node with original index for sorting
#[derive(Clone)]
pub struct IndexedNode {
    pub node: VmessNode,
    pub original_index: usize,
}

/// Application state
pub struct App {
    /// Subscription URL
    pub subscribe_url: Option<String>,
    /// List of vmess nodes (original order)
    nodes: Vec<VmessNode>,
    /// Sorted view of nodes with original indices
    pub sorted_nodes: Vec<IndexedNode>,
    /// Currently selected index in sorted view
    pub selected: usize,
    /// Currently active node's original index
    pub active_node_index: Option<usize>,
    /// Status message to display
    pub status: String,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Whether latency testing is in progress
    pub testing: bool,
    /// Whether subscription refresh is in progress
    pub refreshing: bool,
    /// Whether in URL input mode
    pub input_mode: bool,
    /// Input buffer for URL
    pub input_buffer: String,
    /// Current test type being performed
    pub current_test_type: Option<TestType>,
    /// Error message to display in popup
    pub error_message: Option<String>,
    /// Cancel flag for latency testing
    pub cancel_flag: Arc<AtomicBool>,
    /// Parallel test count
    pub parallel_count: usize,
    /// Xray config file path
    pub xray_config_path: String,
    /// Current sort column
    pub sort_column: SortColumn,
    /// Current sort direction
    pub sort_direction: SortDirection,
}

impl App {
    /// Create a new App instance, loading config from file
    pub fn new(parallel_count: usize, xray_config_path: String) -> Self {
        let config = Config::load();
        let nodes = config.to_vmess_nodes();
        let has_url = config.subscribe_url.is_some();
        let has_nodes = !nodes.is_empty();

        // Load sort settings
        let sort_column = SortColumn::from_str(config.sort_column.as_deref());
        let sort_direction = SortDirection::from_str(config.sort_direction.as_deref());

        // Find active node from xray config
        let active_node_index = read_active_node(&xray_config_path)
            .and_then(|active| find_active_node_index(&nodes, &active));

        let status = if has_url && has_nodes {
            format!(
                "Loaded {} nodes. Press R to refresh, t/T to test.",
                nodes.len()
            )
        } else if has_url {
            "Press R to refresh subscription".to_string()
        } else {
            "Press U to set subscription URL".to_string()
        };

        // Create sorted view
        let mut sorted_nodes: Vec<IndexedNode> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| IndexedNode {
                node: n.clone(),
                original_index: i,
            })
            .collect();

        // Apply saved sort
        apply_sort_to_nodes(&mut sorted_nodes, sort_column, sort_direction);

        // Find selected index in sorted view for active node
        let selected = active_node_index
            .and_then(|ai| sorted_nodes.iter().position(|n| n.original_index == ai))
            .unwrap_or(0);

        Self {
            subscribe_url: config.subscribe_url,
            nodes,
            sorted_nodes,
            selected,
            active_node_index,
            status,
            should_quit: false,
            testing: false,
            refreshing: false,
            input_mode: false,
            input_buffer: String::new(),
            current_test_type: None,
            error_message: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            parallel_count,
            xray_config_path,
            sort_column,
            sort_direction,
        }
    }

    /// Rebuild sorted view from current nodes
    fn rebuild_sorted_view(&mut self) {
        self.sorted_nodes = self
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| IndexedNode {
                node: n.clone(),
                original_index: i,
            })
            .collect();
        self.apply_sort();
    }

    /// Apply current sort settings
    fn apply_sort(&mut self) {
        apply_sort_to_nodes(&mut self.sorted_nodes, self.sort_column, self.sort_direction);
    }

    /// Cycle to next sort column
    pub fn cycle_sort(&mut self) {
        self.sort_column = self.sort_column.next();
        self.sort_direction = SortDirection::Ascending;
        self.apply_sort();
        self.clamp_selection();
        self.save_sort_config();
    }

    /// Toggle sort direction (if no column, start with TCP descending)
    pub fn toggle_sort_direction(&mut self) {
        if self.sort_column == SortColumn::None {
            self.sort_column = SortColumn::Tcp;
            self.sort_direction = SortDirection::Descending;
        } else {
            self.sort_direction = self.sort_direction.toggle();
        }
        self.apply_sort();
        self.save_sort_config();
    }

    /// Clamp selection to valid range
    fn clamp_selection(&mut self) {
        if self.sorted_nodes.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.sorted_nodes.len() {
            self.selected = self.sorted_nodes.len() - 1;
        }
    }

    /// Move selection up
    pub fn select_previous(&mut self) {
        if !self.sorted_nodes.is_empty() && self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        if !self.sorted_nodes.is_empty() && self.selected < self.sorted_nodes.len() - 1 {
            self.selected += 1;
        }
    }

    /// Get the currently selected node
    pub fn selected_node(&self) -> Option<&VmessNode> {
        self.sorted_nodes.get(self.selected).map(|n| &n.node)
    }

    /// Get the original index of the currently selected node
    pub fn selected_original_index(&self) -> Option<usize> {
        self.sorted_nodes.get(self.selected).map(|n| n.original_index)
    }

    /// Update node latency from test result
    pub fn update_latency(&mut self, result: LatencyResult) {
        // Update in original nodes
        if let Some(node) = self.nodes.get_mut(result.index) {
            match result.test_type {
                TestType::Http => node.http_latency = result.latency,
                TestType::Tcp => node.tcp_latency = result.latency,
            }
        }
        // Update in sorted view
        if let Some(indexed) = self
            .sorted_nodes
            .iter_mut()
            .find(|n| n.original_index == result.index)
        {
            match result.test_type {
                TestType::Http => indexed.node.http_latency = result.latency,
                TestType::Tcp => indexed.node.tcp_latency = result.latency,
            }
        }
    }

    /// Set nodes from subscription
    pub fn set_nodes(&mut self, nodes: Vec<VmessNode>) {
        // Try to find active node in the new list
        let active_node_index = read_active_node(&self.xray_config_path)
            .and_then(|active| find_active_node_index(&nodes, &active));

        self.nodes = nodes;
        self.active_node_index = active_node_index;
        self.rebuild_sorted_view();

        // Find selected index in sorted view for active node
        self.selected = active_node_index
            .and_then(|ai| {
                self.sorted_nodes
                    .iter()
                    .position(|n| n.original_index == ai)
            })
            .unwrap_or(0);
    }

    /// Clear all nodes
    pub fn clear_nodes(&mut self) {
        self.nodes.clear();
        self.sorted_nodes.clear();
        self.selected = 0;
        self.active_node_index = None;
    }

    /// Clear HTTP latencies
    pub fn clear_http_latencies(&mut self) {
        for node in &mut self.nodes {
            node.http_latency = LatencyStatus::NotTested;
        }
        for indexed in &mut self.sorted_nodes {
            indexed.node.http_latency = LatencyStatus::NotTested;
        }
    }

    /// Clear TCP latencies
    pub fn clear_tcp_latencies(&mut self) {
        for node in &mut self.nodes {
            node.tcp_latency = LatencyStatus::NotTested;
        }
        for indexed in &mut self.sorted_nodes {
            indexed.node.tcp_latency = LatencyStatus::NotTested;
        }
    }

    /// Set status message
    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
    }

    /// Set error message for popup
    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error_message = Some(error.into());
    }

    /// Clear error message
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Enter URL input mode
    pub fn enter_input_mode(&mut self) {
        self.input_mode = true;
        self.input_buffer = self.subscribe_url.clone().unwrap_or_default();
    }

    /// Exit URL input mode without saving
    pub fn exit_input_mode(&mut self) {
        self.input_mode = false;
        self.input_buffer.clear();
    }

    /// Confirm URL input and save
    pub fn confirm_input(&mut self) {
        let url = self.input_buffer.trim().to_string();
        if !url.is_empty() {
            self.subscribe_url = Some(url);
            self.save_config();
            self.set_status("URL saved. Press R to refresh subscription.");
        }
        self.input_mode = false;
        self.input_buffer.clear();
    }

    /// Save current state to config file
    pub fn save_config(&self) {
        let mut config = Config::load();
        config.subscribe_url = self.subscribe_url.clone();
        config.update_nodes(&self.nodes);
        config.sort_column = self.sort_column.to_str().map(String::from);
        config.sort_direction = if self.sort_column != SortColumn::None {
            Some(self.sort_direction.to_str().to_string())
        } else {
            None
        };
        if let Err(e) = config.save() {
            eprintln!("Failed to save config: {e}");
        }
    }

    /// Save only sort settings to config file
    fn save_sort_config(&self) {
        let mut config = Config::load();
        config.sort_column = self.sort_column.to_str().map(String::from);
        config.sort_direction = if self.sort_column != SortColumn::None {
            Some(self.sort_direction.to_str().to_string())
        } else {
            None
        };
        if let Err(e) = config.save() {
            eprintln!("Failed to save sort config: {e}");
        }
    }

    /// Set active node index after applying a node
    pub fn set_active_node(&mut self, original_index: usize) {
        self.active_node_index = Some(original_index);
    }

    /// Get nodes for cloning (used for latency testing)
    pub fn get_nodes_clone(&self) -> Vec<VmessNode> {
        self.nodes.clone()
    }

    /// Cancel ongoing latency test
    pub fn cancel_test(&mut self) {
        if self.testing {
            self.cancel_flag.store(true, Ordering::SeqCst);
            self.testing = false;
            self.current_test_type = None;
            self.set_status("Latency test cancelled");
            // Reset cancel flag for next test
            self.cancel_flag = Arc::new(AtomicBool::new(false));
        }
    }

    /// Get a clone of the cancel flag
    pub fn get_cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// Re-sort after latency test completes
    pub fn resort(&mut self) {
        if self.sort_column != SortColumn::None {
            self.apply_sort();
        }
    }
}

/// Convert latency status to a sortable key
/// NotTested and TimedOut are sorted to the end
fn latency_sort_key(status: &LatencyStatus) -> (u8, u64) {
    match status {
        LatencyStatus::Success(ms) => (0, *ms),
        LatencyStatus::TimedOut => (1, 0),
        LatencyStatus::NotTested => (2, 0),
    }
}

/// Apply sort to a vector of IndexedNodes
fn apply_sort_to_nodes(
    nodes: &mut [IndexedNode],
    sort_column: SortColumn,
    sort_direction: SortDirection,
) {
    match sort_column {
        SortColumn::None => {
            nodes.sort_by_key(|n| n.original_index);
        }
        SortColumn::Name => {
            nodes.sort_by(|a, b| {
                let cmp = a.node.display_name().cmp(&b.node.display_name());
                if sort_direction == SortDirection::Descending {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        }
        SortColumn::Tcp => {
            nodes.sort_by(|a, b| {
                let cmp = latency_sort_key(&a.node.tcp_latency)
                    .cmp(&latency_sort_key(&b.node.tcp_latency));
                if sort_direction == SortDirection::Descending {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        }
        SortColumn::Http => {
            nodes.sort_by(|a, b| {
                let cmp = latency_sort_key(&a.node.http_latency)
                    .cmp(&latency_sort_key(&b.node.http_latency));
                if sort_direction == SortDirection::Descending {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        }
    }
}
