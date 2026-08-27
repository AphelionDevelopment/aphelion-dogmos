# Numerical invariants

Every externally observable mole count, temperature, volume, heat capacity, pressure input, and intermediate returned to DM is finite. Mole counts and volume are non-negative. Existing public temperature bounds, including the cosmic microwave background floor, remain enforced. Immutable mixtures do not change through mutators.

Merge, remove, transfer, diffusion, and finite-capacity heat conduction conserve total moles or energy within an explicitly named tolerance derived independently of the implementation. A source-free diffusion/conduction step does not create a value outside its connected component's pre-step extrema. Tests use literal/golden fixtures and property inputs; they do not compute expected results with the code under test.

Diffusion topology is reciprocal, duplicate-free, self-edge-free, and cardinal-degree bounded at six including multiz. Invalid topology returns a diagnostic before processing. The current diffusion constant and relaxation budget are gameplay contracts; do not reinterpret the iteration budget as elapsed seconds or tune coefficients as an optimization.

Heat processing receives explicit elapsed seconds. Conductance updates must be stable for supported four- and six-neighbor graphs, unequal/zero/infinite capacities, and high conductivity. Space radiation applies once per elapsed interval. Deterministic inputs require deterministic event order and repeatable same-build values; cross-platform comparisons use documented tolerances where floating-point order differs.

Validate at the Rust domain boundary before mutation. Reject non-finite inputs with caller-legible context. Repair functions zero or clamp invalid state deliberately and invalidate dependent caches; they never hide corruption by leaving a stale value in place.
