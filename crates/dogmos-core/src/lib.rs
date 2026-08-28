pub mod metadata;
pub mod numerics;
pub mod reactions;
pub mod world;

pub use numerics::diffusion::MixtureHandle;

pub const MAX_GAS_SLOTS: usize = 32;
