pub mod frontier;
pub mod metadata;
pub mod numerics;
pub mod reactions;
mod slot_index;
pub mod stage_cursor;
pub mod topology;
mod transaction;
pub mod world;

pub use numerics::diffusion::MixtureHandle;

pub const MAX_GAS_SLOTS: usize = 32;
