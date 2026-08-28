use std::{fs, path::Path};

#[test]
fn generated_bindings_include_the_production_mixture_command_family() {
	let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let source = fs::read_to_string(crate_root.join("src/lib.rs")).unwrap();
	let bindings = fs::read_to_string(crate_root.join("bindings.dm")).unwrap();
	for binding in [
		"/proc/dogmos_gas_metadata_install",
		"/proc/dogmos_mixture_adjust_multiple",
		"/proc/dogmos_mixture_command",
		"/proc/dogmos_mixture_lifecycle_batch",
		"/proc/dogmos_mixture_snapshot",
		"/proc/dogmos_mixture_state_batch",
		"/proc/dogmos_reaction_metadata_install",
		"/proc/dogmos_service_telemetry",
		"/proc/dogmos_simulation_stage",
		"/proc/dogmos_turf_adjacency_batch",
		"/proc/dogmos_turf_heat_adjacency_batch",
		"/proc/dogmos_turf_heat_batch",
		"/proc/dogmos_turf_heat_snapshot",
		"/proc/dogmos_turf_lifecycle_batch",
	] {
		assert!(
			source.contains(&format!("#[auxmacros::bind(\"{binding}\")]")),
			"missing production command export {binding}"
		);
		assert!(
			bindings.contains(&format!("{binding}(")),
			"generated bindings omit production command export {binding}"
		);
	}
}
