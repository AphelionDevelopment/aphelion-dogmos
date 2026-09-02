use std::{
	collections::{BTreeMap, BTreeSet},
	error::Error,
	fmt,
};

pub const GAS_DIFFUSION_CONSTANT: f32 = 0.125;
pub const MAX_CARDINAL_NEIGHBORS: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NodeHandle(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub struct MixtureHandle {
	pub slot: u32,
	pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphNode {
	pub handle: NodeHandle,
	pub generation: u32,
	pub mixture: Option<MixtureHandle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DirectedEdge {
	pub from: NodeHandle,
	pub to: NodeHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeUpsert {
	Inserted,
	Replaced { previous_generation: u32 },
	IgnoredStale { current_generation: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphValidationError {
	DuplicateNode(NodeHandle),
	MissingMixture(NodeHandle),
	UnknownNode(NodeHandle),
	SelfEdge(NodeHandle),
	DuplicateEdge { from: NodeHandle, to: NodeHandle },
	MissingReciprocalEdge { from: NodeHandle, to: NodeHandle },
	DegreeExceeded { handle: NodeHandle, degree: u32 },
	HandleSpaceExceeded,
}

impl fmt::Display for GraphValidationError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::DuplicateNode(handle) => write!(formatter, "duplicate graph node {}", handle.0),
			Self::MissingMixture(handle) => {
				write!(formatter, "graph node {} has no mixture", handle.0)
			}
			Self::UnknownNode(handle) => write!(formatter, "unknown graph node {}", handle.0),
			Self::SelfEdge(handle) => write!(formatter, "graph node {} has a self-edge", handle.0),
			Self::DuplicateEdge { from, to } => {
				write!(formatter, "duplicate graph edge {} -> {}", from.0, to.0)
			}
			Self::MissingReciprocalEdge { from, to } => {
				write!(
					formatter,
					"graph edge {} -> {} is not reciprocal",
					from.0, to.0
				)
			}
			Self::DegreeExceeded { handle, degree } => write!(
				formatter,
				"graph node {} has degree {}, above the supported maximum {}",
				handle.0, degree, MAX_CARDINAL_NEIGHBORS,
			),
			Self::HandleSpaceExceeded => formatter.write_str("graph exceeds the u32 handle space"),
		}
	}
}

impl Error for GraphValidationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffusionError {
	Cancelled,
	UnsupportedDegree(u32),
	StateLength { expected: usize, actual: usize },
	OutputLength { expected: usize, actual: usize },
	InvalidStateValue { index: usize },
	StateSpaceExceeded,
}

impl fmt::Display for DiffusionError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Cancelled => formatter.write_str("diffusion was cancelled"),
			Self::UnsupportedDegree(degree) => write!(
				formatter,
				"diffusion degree {degree} exceeds the supported maximum {MAX_CARDINAL_NEIGHBORS}",
			),
			Self::StateLength { expected, actual } => write!(
				formatter,
				"diffusion state has {actual} values; expected {expected}",
			),
			Self::OutputLength { expected, actual } => write!(
				formatter,
				"diffusion output has {actual} values; expected {expected}",
			),
			Self::InvalidStateValue { index } => {
				write!(
					formatter,
					"diffusion state value {index} is negative or non-finite"
				)
			}
			Self::StateSpaceExceeded => {
				formatter.write_str("diffusion state length overflowed usize")
			}
		}
	}
}

impl Error for DiffusionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffusionGraph {
	nodes: Vec<GraphNode>,
	handle_to_index: BTreeMap<NodeHandle, u32>,
	offsets: Vec<u32>,
	neighbors: Vec<u32>,
}

impl DiffusionGraph {
	#[must_use]
	pub fn node_count(&self) -> usize {
		self.nodes.len()
	}

	#[must_use]
	pub fn degree(&self, handle: NodeHandle) -> Option<u32> {
		let index = *self.handle_to_index.get(&handle)? as usize;
		Some(self.offsets[index + 1] - self.offsets[index])
	}

	fn neighbor_indices(&self, index: usize) -> &[u32] {
		let start = self.offsets[index] as usize;
		let end = self.offsets[index + 1] as usize;
		&self.neighbors[start..end]
	}
}

#[must_use]
pub fn upsert_graph_node(nodes: &mut Vec<GraphNode>, replacement: GraphNode) -> NodeUpsert {
	let Some(existing) = nodes
		.iter_mut()
		.find(|node| node.handle == replacement.handle)
	else {
		nodes.push(replacement);
		return NodeUpsert::Inserted;
	};
	if replacement.generation < existing.generation {
		return NodeUpsert::IgnoredStale {
			current_generation: existing.generation,
		};
	}
	let previous_generation = existing.generation;
	*existing = replacement;
	NodeUpsert::Replaced {
		previous_generation,
	}
}

pub fn validate_graph(
	nodes: &[GraphNode],
	edges: &[DirectedEdge],
) -> Result<DiffusionGraph, GraphValidationError> {
	let mut handle_to_index = BTreeMap::new();
	for (index, node) in nodes.iter().copied().enumerate() {
		if node.mixture.is_none() {
			return Err(GraphValidationError::MissingMixture(node.handle));
		}
		let index = u32::try_from(index).map_err(|_| GraphValidationError::HandleSpaceExceeded)?;
		if handle_to_index.insert(node.handle, index).is_some() {
			return Err(GraphValidationError::DuplicateNode(node.handle));
		}
	}

	let mut edge_set = BTreeSet::new();
	for edge in edges.iter().copied() {
		if !handle_to_index.contains_key(&edge.from) {
			return Err(GraphValidationError::UnknownNode(edge.from));
		}
		if !handle_to_index.contains_key(&edge.to) {
			return Err(GraphValidationError::UnknownNode(edge.to));
		}
		if edge.from == edge.to {
			return Err(GraphValidationError::SelfEdge(edge.from));
		}
		if !edge_set.insert(edge) {
			return Err(GraphValidationError::DuplicateEdge {
				from: edge.from,
				to: edge.to,
			});
		}
	}
	/*
		Build the CSR adjacency with a counting pass and a prefix sum rather than a
		`Vec<Vec<u32>>`. The per-node vector cost one heap allocation (plus its growth reallocs)
		for every node in the graph, which on a full station map is hundreds of thousands of
		allocations every time the turf graph is rebuilt.

		Resolving each edge's endpoints once here also keeps this to a single `handle_to_index`
		lookup per edge, the same as the vector-of-vectors build did.
	*/
	let mut degrees = vec![0_u32; nodes.len()];
	let mut resolved = Vec::with_capacity(edge_set.len());
	for edge in edge_set.iter().copied() {
		if !edge_set.contains(&DirectedEdge {
			from: edge.to,
			to: edge.from,
		}) {
			return Err(GraphValidationError::MissingReciprocalEdge {
				from: edge.from,
				to: edge.to,
			});
		}
		let from = handle_to_index[&edge.from];
		let to = handle_to_index[&edge.to];
		degrees[from as usize] += 1;
		resolved.push((from, to));
	}
	for (index, &degree) in degrees.iter().enumerate() {
		if degree > MAX_CARDINAL_NEIGHBORS {
			return Err(GraphValidationError::DegreeExceeded {
				handle: nodes[index].handle,
				degree,
			});
		}
	}

	let mut offsets = Vec::with_capacity(nodes.len() + 1);
	let mut edge_count = 0_u32;
	offsets.push(0);
	for &degree in &degrees {
		edge_count = edge_count
			.checked_add(degree)
			.ok_or(GraphValidationError::HandleSpaceExceeded)?;
		offsets.push(edge_count);
	}

	let mut neighbors = vec![0_u32; edge_count as usize];
	let mut cursors = degrees;
	cursors.fill(0);
	for (from, to) in resolved {
		let slot = offsets[from as usize] + cursors[from as usize];
		neighbors[slot as usize] = to;
		cursors[from as usize] += 1;
	}
	// Each row holds at most `MAX_CARDINAL_NEIGHBORS` entries, so this keeps the ascending
	// neighbor order the vector-of-vectors build produced without a meaningful sort cost.
	for index in 0..nodes.len() {
		let start = offsets[index] as usize;
		let end = offsets[index + 1] as usize;
		neighbors[start..end].sort_unstable();
	}

	Ok(DiffusionGraph {
		nodes: nodes.to_vec(),
		handle_to_index,
		offsets,
		neighbors,
	})
}

pub fn diffusion_self_weight(degree: u32) -> Result<f32, DiffusionError> {
	if degree > MAX_CARDINAL_NEIGHBORS {
		return Err(DiffusionError::UnsupportedDegree(degree));
	}
	Ok(1.0 - degree as f32 * GAS_DIFFUSION_CONSTANT)
}

pub fn diffusion_step(
	graph: &DiffusionGraph,
	gas_count: u32,
	state: &[f32],
) -> Result<Vec<f32>, DiffusionError> {
	let expected = graph
		.node_count()
		.checked_mul(gas_count as usize)
		.ok_or(DiffusionError::StateSpaceExceeded)?;
	let mut result = vec![0.0; expected];
	diffusion_step_into(graph, gas_count, state, &mut result)?;
	Ok(result)
}

pub fn diffusion_step_into(
	graph: &DiffusionGraph,
	gas_count: u32,
	state: &[f32],
	result: &mut [f32],
) -> Result<(), DiffusionError> {
	diffusion_step_into_cancellable(graph, gas_count, state, result, || false)
}

pub fn diffusion_step_into_cancellable(
	graph: &DiffusionGraph,
	gas_count: u32,
	state: &[f32],
	result: &mut [f32],
	mut should_cancel: impl FnMut() -> bool,
) -> Result<(), DiffusionError> {
	let gas_count = gas_count as usize;
	let expected = graph
		.node_count()
		.checked_mul(gas_count)
		.ok_or(DiffusionError::StateSpaceExceeded)?;
	if state.len() != expected {
		return Err(DiffusionError::StateLength {
			expected,
			actual: state.len(),
		});
	}
	if result.len() != expected {
		return Err(DiffusionError::OutputLength {
			expected,
			actual: result.len(),
		});
	}
	if let Some(index) = state
		.iter()
		.position(|value| !value.is_finite() || *value < 0.0)
	{
		return Err(DiffusionError::InvalidStateValue { index });
	}

	/*
		Node-major with the gas loop innermost, rather than gas-major with the neighbor loop
		innermost. Both visit the same values, but this order walks each neighbor's gas row
		contiguously instead of re-striding the whole state vector once per gas, which keeps the
		working set to one cache line per row and lets the inner loop vectorize.

		The arithmetic sequence per (node, gas) is unchanged - the self term first, then each
		neighbor in the same order - so results stay bit-for-bit identical. That equivalence is
		pinned by `diffusion_matches_the_reference_stencil_bit_for_bit`.
	*/
	for node_index in 0..graph.node_count() {
		if node_index % 64 == 0 && should_cancel() {
			return Err(DiffusionError::Cancelled);
		}
		let adjacent = graph.neighbor_indices(node_index);
		let self_weight = diffusion_self_weight(adjacent.len() as u32)?;
		let row = node_index * gas_count;
		let output_row = &mut result[row..row + gas_count];
		for (output, &value) in output_row.iter_mut().zip(&state[row..row + gas_count]) {
			*output = value * self_weight;
		}
		for &neighbor_index in adjacent {
			let neighbor_row = neighbor_index as usize * gas_count;
			let neighbor_row = &state[neighbor_row..neighbor_row + gas_count];
			for (output, &value) in output_row.iter_mut().zip(neighbor_row) {
				*output += value * GAS_DIFFUSION_CONSTANT;
			}
		}
	}
	Ok(())
}
