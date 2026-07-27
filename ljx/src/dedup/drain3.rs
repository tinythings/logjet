//! Drain3 log template miner — Rust port of the Go reference.
//!
//! Prefix-tree based online log parsing. Each unique log "shape" is
//! assigned to a cluster with a template containing `<*>` wildcards
//! for variable tokens.
//!
//! This port strips persistence, LRU eviction, serialisation, and
//! `ExtractParameters`. `parametrise_numeric_tokens` is always false
//! (stage 3b already handled numerics).

use std::collections::HashMap;

/// Configuration for the Drain algorithm.
pub struct DrainConfig {
    /// Tree depth (minimum 3). First level groups by token count.
    pub depth: i64,
    /// Similarity threshold (0.0–1.0). Higher = stricter matching.
    pub sim_th: f64,
    /// Max child nodes per tree node before falling back to wildcard.
    pub max_children: i64,
    /// Maximum number of clusters.
    pub max_clusters: usize,
    /// Extra delimiters replaced with space before tokenising.
    pub extra_delimiters: Vec<String>,
    /// Route numeric-bearing tokens through wildcard branches in the tree.
    pub parametrize_numeric_tokens: bool,
}

impl Default for DrainConfig {
    fn default() -> Self {
        Self { depth: 4, sim_th: 0.4, max_children: 100, max_clusters: 1000, extra_delimiters: Vec::new(), parametrize_numeric_tokens: true }
    }
}

/// A cluster of log messages sharing a common template.
#[derive(Debug, Clone)]
pub struct LogCluster {
    pub cluster_id: i64,
    pub template_tokens: Vec<String>,
    pub size: i64,
}

impl LogCluster {
    fn new(id: i64, tokens: Vec<String>) -> Self {
        Self { cluster_id: id, template_tokens: tokens, size: 1 }
    }

    /// Reconstruct the template string from tokens.
    pub fn template(&self) -> String {
        self.template_tokens.join(" ")
    }
}

/// Prefix-tree node.
struct Node {
    children: HashMap<String, Node>,
    cluster_ids: Vec<i64>,
}

impl Node {
    fn new() -> Self {
        Self { children: HashMap::new(), cluster_ids: Vec::new() }
    }
}

/// The Drain algorithm state.
pub struct Drain {
    max_node_depth: i64,
    sim_th: f64,
    max_children: i64,
    root: Node,
    extra_delimiters: Vec<String>,
    param_str: String,
    parametrize_numeric_tokens: bool,
    clusters: HashMap<i64, LogCluster>,
    next_id: i64,
}

impl Drain {
    /// Create a new Drain instance from config.
    pub fn new(cfg: DrainConfig) -> Self {
        let max_node_depth = cfg.depth.max(3) - 2;
        Self {
            max_node_depth,
            sim_th: cfg.sim_th,
            max_children: cfg.max_children,
            root: Node::new(),
            extra_delimiters: cfg.extra_delimiters,
            param_str: "<*>".into(),
            parametrize_numeric_tokens: cfg.parametrize_numeric_tokens,
            clusters: HashMap::with_capacity(cfg.max_clusters),
            next_id: 0,
        }
    }

    /// Feed a log message. Returns (cluster_id, is_new_cluster).
    pub fn add_log_message(&mut self, content: &str) -> (i64, bool) {
        let tokens = self.tokenise(content);
        let matched = self.tree_search(&tokens).or_else(|| {
            let all_ids = self.get_cluster_ids_for_seq_len(tokens.len());
            self.fast_match(&all_ids, &tokens, true)
        });

        if let Some(cid) = matched {
            let cluster = self.clusters.get_mut(&cid).unwrap();
            let new_template = create_template(&tokens, &cluster.template_tokens);
            cluster.template_tokens = new_template;
            cluster.size += 1;
            (cid, false)
        } else {
            self.next_id += 1;
            let id = self.next_id;
            let cluster = LogCluster::new(id, tokens);
            self.add_to_prefix_tree(&cluster);
            self.clusters.insert(id, cluster);
            (id, true)
        }
    }

    /// Get all clusters.
    pub fn clusters(&self) -> &HashMap<i64, LogCluster> {
        &self.clusters
    }

    fn tokenise(&self, content: &str) -> Vec<String> {
        let mut s = content.trim().to_string();
        for delim in &self.extra_delimiters {
            s = s.replace(delim.as_str(), " ");
        }
        s.split_whitespace().map(String::from).collect()
    }

    fn tree_search(&self, tokens: &[String]) -> Option<i64> {
        let token_count = tokens.len();
        let count_key = token_count.to_string();

        let first_layer = self.root.children.get(&count_key)?;

        // Empty message — return the single cluster in that group.
        if token_count == 0 {
            return first_layer.cluster_ids.first().copied();
        }

        // Walk prefix tree following token values (or wildcard).
        let mut current = first_layer;

        for (i, token) in tokens.iter().enumerate() {
            let depth = i as i64 + 1;
            if depth >= self.max_node_depth || depth >= token_count as i64 {
                break;
            }
            if let Some(child) = current.children.get(token.as_str()) {
                current = child;
            } else {
                let child = current.children.get(&self.param_str)?;
                current = child;
            }
        }

        self.fast_match(&current.cluster_ids, tokens, false)
    }

    fn fast_match(&self, candidate_ids: &[i64], tokens: &[String], include_params: bool) -> Option<i64> {
        let mut best_sim = -1.0_f64;
        let mut best_param_count = -1_i64;
        let mut best_id: Option<i64> = None;

        for &cid in candidate_ids {
            let Some(cluster) = self.clusters.get(&cid) else { continue };
            let (sim, param_count) = seq_distance(&cluster.template_tokens, tokens, &self.param_str, include_params);
            if sim > best_sim || (sim == best_sim && param_count > best_param_count) {
                best_sim = sim;
                best_param_count = param_count;
                best_id = Some(cid);
            }
        }

        if best_sim >= self.sim_th { best_id } else { None }
    }

    fn add_to_prefix_tree(&mut self, cluster: &LogCluster) {
        let token_count = cluster.template_tokens.len();
        let count_key = token_count.to_string();

        let first_layer = self.root.children.entry(count_key).or_insert_with(Node::new);

        if token_count == 0 {
            first_layer.cluster_ids = vec![cluster.cluster_id];
            return;
        }

        let mut current = first_layer as *mut Node;

        for (i, token) in cluster.template_tokens.iter().enumerate() {
            let depth = i as i64 + 1;
            // Safety: we only hold one mutable reference at a time through
            // the pointer, never aliasing. The tree structure is owned by
            // self and not accessed concurrently.
            let node = unsafe { &mut *current };

            if depth >= self.max_node_depth || depth >= token_count as i64 {
                // Leaf node — attach cluster ID.
                // Clean stale IDs (clusters that were evicted).
                node.cluster_ids.retain(|id| self.clusters.contains_key(id));
                node.cluster_ids.push(cluster.cluster_id);
                break;
            }

            current = if !node.children.contains_key(token.as_str()) && self.parametrize_numeric_tokens && has_numbers(token) {
                node.children.entry(self.param_str.clone()).or_insert_with(Node::new) as *mut Node
            } else if node.children.contains_key(token.as_str()) {
                node.children.get_mut(token.as_str()).unwrap() as *mut Node
            } else if node.children.contains_key(&self.param_str) {
                if (node.children.len() as i64) < self.max_children {
                    node.children.entry(token.clone()).or_insert_with(Node::new) as *mut Node
                } else {
                    node.children.get_mut(&self.param_str).unwrap() as *mut Node
                }
            } else if (node.children.len() as i64 + 1) < self.max_children {
                node.children.entry(token.clone()).or_insert_with(Node::new) as *mut Node
            } else if (node.children.len() as i64 + 1) == self.max_children {
                node.children.entry(self.param_str.clone()).or_insert_with(Node::new) as *mut Node
            } else {
                node.children.get_mut(&self.param_str).unwrap() as *mut Node
            };
        }
    }

    fn get_cluster_ids_for_seq_len(&self, seq_len: usize) -> Vec<i64> {
        let Some(current) = self.root.children.get(&seq_len.to_string()) else { return Vec::new() };
        let mut out = Vec::new();
        collect_cluster_ids(current, &mut out);
        out
    }
}

/// Compute similarity between a template and a token sequence.
/// Returns (similarity_ratio, wildcard_param_count).
fn seq_distance(template: &[String], tokens: &[String], param_str: &str, include_params: bool) -> (f64, i64) {
    if template.len() != tokens.len() {
        return (0.0, 0);
    }
    if template.is_empty() {
        return (1.0, 0);
    }

    let mut sim_tokens: i64 = 0;
    let mut param_count: i64 = 0;

    for (t1, t2) in template.iter().zip(tokens.iter()) {
        if t1 == param_str {
            param_count += 1;
        } else if t1 == t2 {
            sim_tokens += 1;
        }
    }

    if include_params {
        sim_tokens += param_count;
    }

    (sim_tokens as f64 / template.len() as f64, param_count)
}

/// Merge two token sequences into a template: matching tokens preserved,
/// mismatches become `<*>`.
fn create_template(seq1: &[String], seq2: &[String]) -> Vec<String> {
    seq2.iter().zip(seq1.iter()).map(|(t2, t1)| if t1 == t2 { t2.clone() } else { "<*>".into() }).collect()
}

fn has_numbers(token: &str) -> bool {
    token.bytes().any(|b| b.is_ascii_digit())
}

fn collect_cluster_ids(node: &Node, out: &mut Vec<i64>) {
    out.extend(node.cluster_ids.iter().copied());
    for child in node.children.values() {
        collect_cluster_ids(child, out);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dedup/drain3_utst.rs"]
mod drain3_utst;
