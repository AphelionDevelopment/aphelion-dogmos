use std::{error::Error, fmt};

pub const BASE_HEAT_STEP_SECONDS: f32 = 0.5;
pub const BYOND_INFINITY_THRESHOLD: f32 = 1.0e30;
pub const MAX_CONDUCTION_SUBSTEPS: u32 = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConductionStats {
	pub substeps: u32,
	pub edges_applied: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConductionError {
	Cancelled,
	LengthMismatch,
	InvalidElapsedTime,
	InvalidTemperature { index: usize },
	InvalidConductivity { index: usize },
	InvalidCapacity { index: usize },
	UnknownNode(u32),
	SelfEdge(u32),
	DuplicateEdge { first: u32, second: u32 },
	TooManySubsteps,
}

impl fmt::Display for ConductionError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Cancelled => formatter.write_str("conduction was cancelled"),
			Self::LengthMismatch => formatter.write_str(
				"temperature, conductivity, and heat-capacity slices must have equal lengths",
			),
			Self::InvalidElapsedTime => {
				formatter.write_str("elapsed heat time must be finite and non-negative")
			}
			Self::InvalidTemperature { index } => {
				write!(formatter, "temperature {index} is non-finite")
			}
			Self::InvalidConductivity { index } => write!(
				formatter,
				"thermal conductivity {index} is negative or non-finite",
			),
			Self::InvalidCapacity { index } => {
				write!(
					formatter,
					"heat capacity {index} is not finite and positive"
				)
			}
			Self::UnknownNode(handle) => write!(formatter, "heat edge references node {handle}"),
			Self::SelfEdge(handle) => write!(formatter, "heat node {handle} has a self-edge"),
			Self::DuplicateEdge { first, second } => {
				write!(formatter, "duplicate heat edge {first} -- {second}")
			}
			Self::TooManySubsteps => write!(
				formatter,
				"heat step requires more than {MAX_CONDUCTION_SUBSTEPS} substeps",
			),
		}
	}
}

impl Error for ConductionError {}

fn validate_values(
	temperatures: &[f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	seconds_per_tick: f32,
) -> Result<(), ConductionError> {
	if temperatures.len() != conductivities.len() || temperatures.len() != heat_capacities.len() {
		return Err(ConductionError::LengthMismatch);
	}
	if !seconds_per_tick.is_finite() || seconds_per_tick < 0.0 {
		return Err(ConductionError::InvalidElapsedTime);
	}
	for (index, &temperature) in temperatures.iter().enumerate() {
		if !temperature.is_finite() {
			return Err(ConductionError::InvalidTemperature { index });
		}
	}
	for (index, &conductivity) in conductivities.iter().enumerate() {
		if !conductivity.is_finite() || conductivity < 0.0 {
			return Err(ConductionError::InvalidConductivity { index });
		}
	}
	for (index, &capacity) in heat_capacities.iter().enumerate() {
		if !capacity.is_finite() || capacity <= 0.0 {
			return Err(ConductionError::InvalidCapacity { index });
		}
	}
	Ok(())
}

fn canonical_edges(
	edges: &[(u32, u32)],
	node_count: usize,
) -> Result<Vec<(u32, u32)>, ConductionError> {
	let mut canonical = Vec::with_capacity(edges.len());
	for &(first, second) in edges {
		if first as usize >= node_count {
			return Err(ConductionError::UnknownNode(first));
		}
		if second as usize >= node_count {
			return Err(ConductionError::UnknownNode(second));
		}
		if first == second {
			return Err(ConductionError::SelfEdge(first));
		}
		canonical.push(if first < second {
			(first, second)
		} else {
			(second, first)
		});
	}
	canonical.sort_unstable();
	for pair in canonical.windows(2) {
		if pair[0] == pair[1] {
			return Err(ConductionError::DuplicateEdge {
				first: pair[0].0,
				second: pair[0].1,
			});
		}
	}
	Ok(canonical)
}

fn harmonic_capacity(first: f32, second: f32) -> f32 {
	let first_infinite = first >= BYOND_INFINITY_THRESHOLD;
	let second_infinite = second >= BYOND_INFINITY_THRESHOLD;
	if first_infinite && second_infinite {
		return 0.0;
	}
	if first_infinite {
		return second;
	}
	if second_infinite {
		return first;
	}
	let (smaller, larger) = if first < second {
		(first, second)
	} else {
		(second, first)
	};
	smaller / (1.0 + smaller / larger)
}

pub fn heat_row_weight(
	first_conductivity: f32,
	second_conductivity: f32,
	first_capacity: f32,
	second_capacity: f32,
) -> Result<f32, ConductionError> {
	for (index, conductivity) in [first_conductivity, second_conductivity]
		.into_iter()
		.enumerate()
	{
		if !conductivity.is_finite() || conductivity < 0.0 {
			return Err(ConductionError::InvalidConductivity { index });
		}
	}
	for (index, capacity) in [first_capacity, second_capacity].into_iter().enumerate() {
		if !capacity.is_finite() || capacity <= 0.0 {
			return Err(ConductionError::InvalidCapacity { index });
		}
	}
	if first_capacity >= BYOND_INFINITY_THRESHOLD {
		return Ok(0.0);
	}
	Ok(first_conductivity.min(second_conductivity)
		* harmonic_capacity(first_capacity, second_capacity)
		/ first_capacity)
}

fn row_weights(
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
) -> Result<Vec<f32>, ConductionError> {
	let mut row_sums = vec![0.0; conductivities.len()];
	for &(first, second) in edges {
		let first_index = first as usize;
		let second_index = second as usize;
		row_sums[first_index] += heat_row_weight(
			conductivities[first_index],
			conductivities[second_index],
			heat_capacities[first_index],
			heat_capacities[second_index],
		)?;
		row_sums[second_index] += heat_row_weight(
			conductivities[second_index],
			conductivities[first_index],
			heat_capacities[second_index],
			heat_capacities[first_index],
		)?;
	}
	Ok(row_sums)
}

fn required_substeps_from_edges(
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	seconds_per_tick: f32,
) -> Result<u32, ConductionError> {
	let elapsed_scale = seconds_per_tick / BASE_HEAT_STEP_SECONDS;
	let maximum_scaled_sum = row_weights(conductivities, heat_capacities, edges)?
		.into_iter()
		.fold(0.0_f32, f32::max)
		* elapsed_scale;
	if !maximum_scaled_sum.is_finite() || maximum_scaled_sum > MAX_CONDUCTION_SUBSTEPS as f32 {
		return Err(ConductionError::TooManySubsteps);
	}
	Ok((maximum_scaled_sum.ceil() as u32).max(1))
}

pub fn required_conduction_substeps(
	temperatures: &[f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	seconds_per_tick: f32,
) -> Result<u32, ConductionError> {
	validate_values(
		temperatures,
		conductivities,
		heat_capacities,
		seconds_per_tick,
	)?;
	let canonical = canonical_edges(edges, temperatures.len())?;
	required_substeps_from_edges(
		conductivities,
		heat_capacities,
		&canonical,
		seconds_per_tick,
	)
}

fn apply_conduction_substep(
	temperatures: &mut [f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	normalized_scale: f32,
) -> Result<(), ConductionError> {
	for &(first, second) in edges {
		let first_index = first as usize;
		let second_index = second as usize;
		let difference = temperatures[second_index] - temperatures[first_index];
		let first_weight = heat_row_weight(
			conductivities[first_index],
			conductivities[second_index],
			heat_capacities[first_index],
			heat_capacities[second_index],
		)? * normalized_scale;
		let second_weight = heat_row_weight(
			conductivities[second_index],
			conductivities[first_index],
			heat_capacities[second_index],
			heat_capacities[first_index],
		)? * normalized_scale;
		temperatures[first_index] += difference * first_weight;
		temperatures[second_index] -= difference * second_weight;
	}
	Ok(())
}

pub fn conduction_substep(
	temperatures: &mut [f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	normalized_scale: f32,
) -> Result<(), ConductionError> {
	validate_values(
		temperatures,
		conductivities,
		heat_capacities,
		normalized_scale,
	)?;
	let canonical = canonical_edges(edges, temperatures.len())?;
	apply_conduction_substep(
		temperatures,
		conductivities,
		heat_capacities,
		&canonical,
		normalized_scale,
	)
}

pub fn conduction_step(
	temperatures: &mut [f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	seconds_per_tick: f32,
) -> Result<ConductionStats, ConductionError> {
	conduction_step_cancellable(
		temperatures,
		conductivities,
		heat_capacities,
		edges,
		seconds_per_tick,
		|| false,
	)
}

pub fn conduction_step_cancellable(
	temperatures: &mut [f32],
	conductivities: &[f32],
	heat_capacities: &[f32],
	edges: &[(u32, u32)],
	seconds_per_tick: f32,
	mut should_cancel: impl FnMut() -> bool,
) -> Result<ConductionStats, ConductionError> {
	validate_values(
		temperatures,
		conductivities,
		heat_capacities,
		seconds_per_tick,
	)?;
	let canonical = canonical_edges(edges, temperatures.len())?;
	let substeps = required_substeps_from_edges(
		conductivities,
		heat_capacities,
		&canonical,
		seconds_per_tick,
	)?;
	let normalized_scale = (seconds_per_tick / BASE_HEAT_STEP_SECONDS) / substeps as f32;
	for _ in 0..substeps {
		if should_cancel() {
			return Err(ConductionError::Cancelled);
		}
		apply_conduction_substep(
			temperatures,
			conductivities,
			heat_capacities,
			&canonical,
			normalized_scale,
		)?;
	}
	Ok(ConductionStats {
		substeps,
		edges_applied: canonical.len() as u64 * u64::from(substeps),
	})
}
