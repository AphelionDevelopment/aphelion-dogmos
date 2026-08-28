use std::{fs, path::Path};

#[test]
fn production_service_lifecycle_exports_are_distinct_from_benchmarks() {
	let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let source = fs::read_to_string(crate_root.join("src/lib.rs")).unwrap();
	let bindings = fs::read_to_string(crate_root.join("bindings.dm")).unwrap();
	for binding in [
		"/proc/dogmos_abi_version",
		"/proc/dogmos_protocol_version",
		"/proc/dogmos_service_start",
		"/proc/dogmos_service_health",
		"/proc/dogmos_service_pid",
		"/proc/dogmos_service_world_generation",
		"/proc/dogmos_service_shutdown",
		"/proc/dogmos_source_revision",
		"/proc/dogmos_feature_fingerprint",
	] {
		assert!(
			source.contains(&format!("#[auxmacros::bind(\"{binding}\")]")),
			"missing production lifecycle export {binding}"
		);
		assert!(
			bindings.contains(&format!("{binding}(")),
			"generated bindings omit production lifecycle export {binding}"
		);
	}
}

#[test]
fn production_bindings_exclude_opt_in_diagnostics() {
	let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let bindings = fs::read_to_string(crate_root.join("bindings.dm")).unwrap();
	assert!(!bindings.contains("/proc/dogmos_ipc_benchmark_"));
}
