
use std::collections::HashMap;
use petgraph::Undirected;
use petgraph::graph::{Graph, NodeIndex};
use hexx::Hex;

// Using adjacency list representation for the subgraphs because they will typically be
// very small, so I expect this to be fast and easier to resize/reset without allocating.
type NeighborhoodGraph = Graph<(), (), Undirected, usize>;


#[derive(Copy, Clone, PartialEq)]
struct DfsInfo {
    parent: NodeIndex<usize>,
    depth: u32,
    low: u32,
}

impl Default for DfsInfo {
    fn default() -> Self {
        return Self {
            parent: NodeIndex::end(),
            depth: 0,
            low: 0,
        };
    }
}

pub struct ConnectivityChecker {
    subgraph: NeighborhoodGraph,
    hex_to_subgraph: HashMap<Hex, NodeIndex<usize>>,
    dfsinfo: HashMap<NodeIndex<usize>, DfsInfo>,
}

impl ConnectivityChecker {
    pub fn new() -> Self {
        let subgraph: NeighborhoodGraph = Graph::default();

        return Self {
            subgraph: subgraph,
            hex_to_subgraph: HashMap::new(),
            dfsinfo: HashMap::new(),
        };
    }

    pub fn hex_isarticulation<F>(&mut self, root: Hex, hexcell: F, cell: u32) -> bool where F: Fn(Hex) -> u32 {
        self.construct_hex_subgraph(root, hexcell, cell);
        self.dfsinfo.clear();
        return is_articulation_dfs(
            &self.subgraph,
            &mut self.dfsinfo,
            self.hex_to_subgraph[&root],
            NodeIndex::end(), 0);
    }

    fn construct_hex_subgraph<F>(&mut self, root: Hex, hexcell: F, cell: u32) where F: Fn(Hex) -> u32 {
        self.subgraph.clear();
        self.hex_to_subgraph.clear();

        let i_idx = self.subgraph.add_node(());
        self.hex_to_subgraph.insert(root, i_idx);

        for neighbor in root.all_neighbors() {
            let neighbor_cell = hexcell(neighbor);
            if cell == neighbor_cell {
                self.hex_to_subgraph.entry(neighbor).or_insert_with(|| {
                    let j_idx = self.subgraph.add_node(());
                    j_idx
                });
            }
        }

        // add edges from i to neighbors, and neighbors' neighbors
        for neighbor in root.all_neighbors() {
            if !self.hex_to_subgraph.contains_key(&neighbor) {
                continue;
            }

            self.subgraph.add_edge(
                self.hex_to_subgraph[&root],
                self.hex_to_subgraph[&neighbor], ());

            for k in neighbor.all_neighbors() {
                let k_cell = hexcell(k);
                if self.hex_to_subgraph.contains_key(&k) && k_cell == cell {
                    self.subgraph.add_edge(
                        self.hex_to_subgraph[&neighbor],
                        self.hex_to_subgraph[&k], ());
                }
            }
        }
    }


}

// Recursive traversal function to find articulation points
fn is_articulation_dfs(
    subgraph: &NeighborhoodGraph,
    dfsinfo: &mut HashMap<NodeIndex<usize>, DfsInfo>,
    i: NodeIndex<usize>,
    parent: NodeIndex<usize>,
    depth: u32) -> bool
{
    dfsinfo.entry(i).or_insert_with(|| 
        DfsInfo{
            parent,
            depth,
            low: depth
        });

    let mut child_count = 0;
    let mut is_articulation = false;

    for j in subgraph.neighbors(i) {
        if let Some(j_info_depth) = dfsinfo.get(&j).map(|j_info| j_info.depth) {
            if j != parent {
                let mut i_info = dfsinfo.get_mut(&i).unwrap();
                i_info.low = i_info.low.min(j_info_depth);
            }
        } else {
            // j is unvisited
            is_articulation_dfs(subgraph, dfsinfo, j, i, depth + 1);
            let j_info_low = dfsinfo[&j].low;
            child_count += 1;

            let mut i_info = dfsinfo.get_mut(&i).unwrap();
            if j_info_low >= i_info.depth {
                is_articulation = true;
            }
            i_info.low = i_info.low.min(j_info_low);
        }

        let i_info = &dfsinfo[&i];

        if i_info.parent == NodeIndex::end() {
            is_articulation = child_count > 1;
        }
    }

    return is_articulation;
}
