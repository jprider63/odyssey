use daggy::Walker;
use std::cmp;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

pub mod v0;

/// Trait that ECG headers (nodes?) must implement.
pub trait ECGHeader {
    type HeaderId: Ord + Copy + Debug;

    /// Return the parents ids of a node. If an empty slice is returned, the root node is the
    /// parent.
    fn get_parent_ids(&self) -> &[Self::HeaderId];

    fn validate_header(&self, header_id: Self::HeaderId) -> bool;
}

#[derive(Clone, Debug)]
struct NodeInfo<Header> {
    /// The index of this node in the dependency graph.
    graph_index: daggy::NodeIndex,
    /// The (minimum) depth of this node in the dependency graph.
    depth: u64,
    /// The header this node is storing.
    header: Header,
}

#[derive(Clone, Debug)]
pub struct State<Header: ECGHeader> {
    dependency_graph: daggy::stable_dag::StableDag<Header::HeaderId, ()>, // JP: Hold the operations? Depth? Do we need StableDag?

    /// Nodes at the top of the DAG that depend on the initial state.
    root_nodes: BTreeSet<Header::HeaderId>,

    /// Mapping from header ids to node indices.
    node_info_map: BTreeMap<Header::HeaderId, NodeInfo<Header>>,

    /// Tips of the ECG (hashes of their headers).
    tips: BTreeSet<Header::HeaderId>,
}

impl<Header: ECGHeader> State<Header> {
    pub fn new() -> State<Header> {
        let mut dependency_graph = daggy::stable_dag::StableDag::new();

        State {
            dependency_graph,
            root_nodes: BTreeSet::new(),
            node_info_map: BTreeMap::new(),
            tips: BTreeSet::new(),
        }
    }

    pub fn tips(&self) -> &BTreeSet<Header::HeaderId> {
        &self.tips
    }

    /// Returns the parents of the given node (with their depths) if it exists. If the returned array is
    /// empty, the node is a root node.
    pub fn get_parents_with_depth(
        &self,
        h: &Header::HeaderId,
    ) -> Option<Vec<(u64, Header::HeaderId)>> {
        let node_info = self.node_info_map.get(h)?;
        self.dependency_graph
            .parents(node_info.graph_index)
            .iter(&self.dependency_graph)
            .map(|(_, parent_idx)| {
                self.dependency_graph
                    .node_weight(parent_idx)
                    .and_then(|parent_id| {
                        self.node_info_map
                            .get(parent_id)
                            .map(|i| (i.depth, *parent_id))
                    })
            })
            .try_collect()
    }

    /// Returns the parents of the given node if it exists. If the returned array is
    /// empty, the node is a root node.
    pub fn get_parents(&self, h: &Header::HeaderId) -> Option<Vec<Header::HeaderId>> {
        let node_info = self.node_info_map.get(h)?;
        self.dependency_graph
            .parents(node_info.graph_index)
            .iter(&self.dependency_graph)
            .map(|(_, parent_idx)| self.dependency_graph.node_weight(parent_idx).map(|i| *i))
            .try_collect()
    }

    /// Returns the children of the given node (with their depths) if it exists. If the returned array is
    /// empty, the node is a leaf node.
    pub fn get_children_with_depth(
        &self,
        h: &Header::HeaderId,
    ) -> Option<Vec<(u64, Header::HeaderId)>> {
        let node_info = self.node_info_map.get(h)?;
        self.dependency_graph
            .children(node_info.graph_index)
            .iter(&self.dependency_graph)
            .map(|(_, child_idx)| {
                self.dependency_graph
                    .node_weight(child_idx)
                    .and_then(|child_id| {
                        self.node_info_map
                            .get(child_id)
                            .map(|i| (i.depth, *child_id))
                    })
            })
            .try_collect()
    }

    // pub fn get_children(&self, n:&HeaderId) -> Vec<HeaderId> {
    //     unimplemented!{}
    // }

    pub fn contains(&self, h: &Header::HeaderId) -> bool {
        if let Some(_node_info) = self.node_info_map.get(h) {
            true
        } else {
            false
        }
    }

    pub fn get_header(&self, n: &Header::HeaderId) -> Option<&Header> {
        self.node_info_map.get(n).map(|i| &i.header)
    }

    pub fn get_header_depth(&self, n: &Header::HeaderId) -> Option<u64> {
        self.node_info_map.get(n).map(|i| i.depth)
    }

    pub fn insert_header(&mut self, header_id: Header::HeaderId, header: Header) -> bool {
        // Validate header.
        if !header.validate_header(header_id) {
            return false;
        }

        // Check that the header is not already in the dependency_graph.
        if self.node_info_map.contains_key(&header_id) {
            return false;
        }

        let parents = header.get_parent_ids();
        let (parent_idxs, depth) = if parents.is_empty() {
            let is_new_insert = self.root_nodes.insert(header_id);
            // Check if it already existed.
            if !is_new_insert {
                // TODO: Log that the state is corrupt. Invariant violated that
                // `root_nodes` is a subset of `node_idx_map.keys()`.
                return false;
            }

            (vec![], 1)
        } else {
            let mut depth = u64::MAX;
            if let Some(parent_idxs) = parents
                .iter()
                .map(|parent_id| {
                    self.node_info_map.get(&parent_id).map(|i| {
                        depth = cmp::min(depth, i.depth);
                        i.graph_index
                    })
                })
                .try_collect::<Vec<daggy::NodeIndex>>()
            {
                (parent_idxs, depth + 1)
            } else {
                return false;
            }
        };

        // Update tip if any of the parents where previously a tip.
        let any_parent_was_tip = parents.iter().any(|parent_id| self.tips.remove(parent_id));
        if any_parent_was_tip {
            self.tips.insert(header_id);
        }

        // Insert node and store its index in `node_idx_map`.
        // JP: We really want an `add_child` function that takes multiple parents.
        let graph_index = self.dependency_graph.add_node(header_id);
        let node_info = NodeInfo {
            graph_index: graph_index.clone(),
            depth,
            header,
        };
        if let Err(_) = self.node_info_map.try_insert(header_id, node_info) {
            // TODO: Should be unreachable. Log this.
            return false;
        }

        // Insert edges.
        if let Err(_) = self.dependency_graph.add_edges(
            parent_idxs
                .into_iter()
                .map(|parent_idx| (parent_idx, graph_index, ())),
        ) {
            // TODO: Unreachable? Log this.
            return false;
        }

        true
    }
}

/// Tests whether two ecg states have the same DAG.
pub(crate) fn equal_dags<Header: ECGHeader>(l: &State<Header>, r: &State<Header>) -> bool {
    unimplemented!()
}
