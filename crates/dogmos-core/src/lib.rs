pub mod frontier;
pub mod metadata;
pub mod numerics;
pub mod reactions;
pub mod stage_cursor;
pub mod topology;
pub mod world;

pub use numerics::diffusion::MixtureHandle;

pub const MAX_GAS_SLOTS: usize = 32;
