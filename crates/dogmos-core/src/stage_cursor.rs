use crate::world::WorldStage;

pub const MAX_STAGE_WORK_LIMIT: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageChunkRequest {
	pub stage: WorldStage,
	pub frontier_epoch: u64,
	pub stage_epoch: u64,
	pub work_limit: u32,
	pub seconds_per_tick: f64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StageChunkResult {
	pub work_items: u32,
	pub callback_events: u32,
	pub pending: bool,
	pub remaining_estimate: u32,
	pub produced_equalize_seeds: u32,
	pub produced_group_seeds: u32,
	pub produced_heat_seeds: u32,
}

pub(crate) struct StageCursor {
	pub(crate) stage: WorldStage,
	pub(crate) frontier_epoch: u64,
	pub(crate) stage_epoch: u64,
	pub(crate) seconds_per_tick_bits: u64,
	pub(crate) topology_revision: u64,
	pub(crate) next_frontier_index: u32,
}

impl StageCursor {
	pub(crate) fn new(request: StageChunkRequest, topology_revision: u64) -> Self {
		Self {
			stage: request.stage,
			frontier_epoch: request.frontier_epoch,
			stage_epoch: request.stage_epoch,
			seconds_per_tick_bits: request.seconds_per_tick.to_bits(),
			topology_revision,
			next_frontier_index: 0,
		}
	}

	pub(crate) fn matches(&self, request: StageChunkRequest) -> bool {
		self.stage == request.stage
			&& self.frontier_epoch == request.frontier_epoch
			&& self.stage_epoch == request.stage_epoch
			&& self.seconds_per_tick_bits == request.seconds_per_tick.to_bits()
	}
}
