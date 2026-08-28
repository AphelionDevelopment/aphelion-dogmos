use std::{
	collections::BTreeSet,
	fs,
	path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
struct Migration {
	legacy_binding: &'static str,
	shim_export: &'static str,
	verification: &'static str,
}

const MIXTURE_COMMAND_BINDINGS: &[&str] = &[
	"/datum/gas_mixture/proc/__adjust_moles",
	"/datum/gas_mixture/proc/__adjust_moles_temp",
	"/datum/gas_mixture/proc/__adjust_multi",
	"/datum/gas_mixture/proc/__get_moles",
	"/datum/gas_mixture/proc/__merge",
	"/datum/gas_mixture/proc/__partial_heat_capacity",
	"/datum/gas_mixture/proc/__react",
	"/datum/gas_mixture/proc/__remove",
	"/datum/gas_mixture/proc/__remove_by_flag",
	"/datum/gas_mixture/proc/__remove_ratio",
	"/datum/gas_mixture/proc/__set_moles",
	"/datum/gas_mixture/proc/add",
	"/datum/gas_mixture/proc/adjust_heat",
	"/datum/gas_mixture/proc/clear",
	"/datum/gas_mixture/proc/compare",
	"/datum/gas_mixture/proc/copy_from",
	"/datum/gas_mixture/proc/divide",
	"/datum/gas_mixture/proc/equalize_with",
	"/datum/gas_mixture/proc/get_by_flag",
	"/datum/gas_mixture/proc/get_fuel_amount",
	"/datum/gas_mixture/proc/get_oxidation_power",
	"/datum/gas_mixture/proc/heat_capacity",
	"/datum/gas_mixture/proc/is_immutable",
	"/datum/gas_mixture/proc/mark_immutable",
	"/datum/gas_mixture/proc/multiply",
	"/datum/gas_mixture/proc/return_pressure",
	"/datum/gas_mixture/proc/return_temperature",
	"/datum/gas_mixture/proc/return_volume",
	"/datum/gas_mixture/proc/scrub_into",
	"/datum/gas_mixture/proc/set_min_heat_capacity",
	"/datum/gas_mixture/proc/set_temperature",
	"/datum/gas_mixture/proc/set_volume",
	"/datum/gas_mixture/proc/share_ratio",
	"/datum/gas_mixture/proc/subtract",
	"/datum/gas_mixture/proc/temperature_share",
	"/datum/gas_mixture/proc/thermal_energy",
	"/datum/gas_mixture/proc/total_moles",
	"/datum/gas_mixture/proc/transfer_ratio_to",
	"/datum/gas_mixture/proc/transfer_to",
	"/proc/equalize_all_gases_in_list",
];

const MIXTURE_LIFECYCLE_BINDINGS: &[&str] = &[
	"/datum/gas_mixture/proc/__gasmixture_register",
	"/datum/gas_mixture/proc/__gasmixture_unregister",
];

const MIXTURE_SNAPSHOT_BINDINGS: &[&str] = &["/datum/gas_mixture/proc/__get_gases"];

const SIMULATION_STAGE_BINDINGS: &[&str] = &[
	"/datum/controller/subsystem/air/proc/finish_turf_processing_auxtools",
	"/datum/controller/subsystem/air/proc/process_excited_groups_auxtools",
	"/datum/controller/subsystem/air/proc/process_turf_equalize_auxtools",
	"/datum/controller/subsystem/air/proc/process_turf_heat",
	"/datum/controller/subsystem/air/proc/process_turfs_auxtools",
	"/datum/controller/subsystem/air/proc/thread_running",
];

const MIXTURE_STATE_BINDINGS: &[&str] = &["/datum/gas_mixture/proc/__auxtools_parse_gas_string"];
const GAS_METADATA_BINDINGS: &[&str] = &["/proc/_auxtools_register_gas", "/proc/finalize_gas_refs"];
const REACTION_METADATA_BINDINGS: &[&str] =
	&["/datum/controller/subsystem/air/proc/auxtools_update_reactions"];

const TURF_LIFECYCLE_BINDINGS: &[&str] = &["/turf/proc/update_air_ref"];
const TURF_ADJACENCY_BINDINGS: &[&str] = &["/turf/proc/__update_auxtools_turf_adjacency_info"];
const TURF_HEAT_BINDINGS: &[&str] = &[
	"/turf/proc/__dogmos_heat_temperature",
	"/turf/proc/__set_temperature",
	"/turf/return_temperature",
];

const CALLBACK_BINDINGS: &[&str] = &["/proc/process_atmos_callbacks"];

const SERVICE_START_BINDINGS: &[&str] = &["/proc/auxtools_atmos_init"];
const SERVICE_SHUTDOWN_BINDINGS: &[&str] = &["/proc/dogmos_shutdown"];

const TELEMETRY_BINDINGS: &[&str] = &[
	"/datum/controller/subsystem/air/proc/get_amt_gas_mixes",
	"/datum/controller/subsystem/air/proc/get_max_gas_mixes",
	"/proc/dogmos_callback_enqueue_failures",
	"/proc/dogmos_ffi_panic_count",
	"/proc/dogmos_heat_graph_count",
	"/proc/dogmos_heat_registration_total",
	"/proc/dogmos_perf_set_detailed",
	"/proc/dogmos_perf_snapshot",
	"/proc/dogmos_reaction_count",
	"/proc/dogmos_space_boundary_count",
];

const MIGRATION_FAMILIES: &[(&[&str], &str, &str)] = &[
	(
		MIXTURE_COMMAND_BINDINGS,
		"/proc/dogmos_mixture_command",
		"crates/dogmos-server/tests/world_transcript.rs",
	),
	(
		MIXTURE_LIFECYCLE_BINDINGS,
		"/proc/dogmos_mixture_lifecycle_batch",
		"crates/dogmos-server/tests/control_plane.rs",
	),
	(
		MIXTURE_SNAPSHOT_BINDINGS,
		"/proc/dogmos_mixture_snapshot",
		"crates/dogmos-byond/tests/production_commands.rs",
	),
	(
		SIMULATION_STAGE_BINDINGS,
		"/proc/dogmos_simulation_stage",
		"crates/dogmos-core/tests/world_state.rs",
	),
	(
		MIXTURE_STATE_BINDINGS,
		"/proc/dogmos_mixture_state_batch",
		"crates/dogmos-byond/tests/production_commands.rs",
	),
	(
		GAS_METADATA_BINDINGS,
		"/proc/dogmos_gas_metadata_install",
		"crates/dogmos-server/tests/control_plane.rs",
	),
	(
		REACTION_METADATA_BINDINGS,
		"/proc/dogmos_reaction_metadata_install",
		"crates/dogmos-server/tests/control_plane.rs",
	),
	(
		TURF_LIFECYCLE_BINDINGS,
		"/proc/dogmos_turf_lifecycle_batch",
		"crates/dogmos-server/tests/control_plane.rs",
	),
	(
		TURF_ADJACENCY_BINDINGS,
		"/proc/dogmos_turf_adjacency_batch",
		"crates/dogmos-server/tests/control_plane.rs",
	),
	(
		TURF_HEAT_BINDINGS,
		"/proc/dogmos_turf_heat_batch",
		"crates/dogmos-core/tests/world_state.rs",
	),
	(
		CALLBACK_BINDINGS,
		"/proc/dogmos_callback_drain",
		"crates/dogmos-server/tests/continuations.rs",
	),
	(
		SERVICE_START_BINDINGS,
		"/proc/dogmos_service_start",
		"crates/dogmos-byond/tests/service_lifecycle.rs",
	),
	(
		SERVICE_SHUTDOWN_BINDINGS,
		"/proc/dogmos_service_shutdown",
		"crates/dogmos-byond/tests/service_lifecycle.rs",
	),
	(
		TELEMETRY_BINDINGS,
		"/proc/dogmos_service_telemetry",
		"crates/dogmos-protocol/tests/telemetry.rs",
	),
];

fn inventory() -> Vec<Migration> {
	MIGRATION_FAMILIES
		.iter()
		.flat_map(|(bindings, shim_export, verification)| {
			bindings.iter().map(move |legacy_binding| Migration {
				legacy_binding,
				shim_export,
				verification,
			})
		})
		.collect()
}

fn collect_rust_files(directory: &Path, output: &mut Vec<PathBuf>) {
	for entry in fs::read_dir(directory).unwrap() {
		let path = entry.unwrap().path();
		if path.is_dir() {
			collect_rust_files(&path, output);
		} else if path.extension().is_some_and(|extension| extension == "rs") {
			output.push(path);
		}
	}
}

fn declared_legacy_bindings() -> BTreeSet<String> {
	let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
	let mut rust_files = Vec::new();
	collect_rust_files(&repository.join("src"), &mut rust_files);
	let mut bindings = BTreeSet::new();
	for path in rust_files {
		let source = fs::read_to_string(path).unwrap();
		for line in source.lines().filter(|line| {
			line.contains("#[auxmacros::bind(\"") || line.contains("#[auxmacros::bind_raw_args(\"")
		}) {
			let start = line.find('"').unwrap() + 1;
			let end = line[start..].find('"').unwrap() + start;
			assert!(bindings.insert(line[start..end].to_owned()));
		}
	}
	bindings
}

#[test]
fn every_legacy_binding_has_one_documented_service_migration() {
	let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
	let declared = declared_legacy_bindings();
	let migrations = inventory();
	let classified = migrations
		.iter()
		.map(|migration| migration.legacy_binding.to_owned())
		.collect::<BTreeSet<_>>();

	assert_eq!(declared.len(), 71);
	assert_eq!(migrations.len(), 71);
	assert_eq!(classified.len(), migrations.len(), "duplicate migration");
	assert_eq!(classified, declared);
	for migration in migrations {
		assert!(migration.shim_export.starts_with("/proc/dogmos_"));
		assert!(migration.verification.ends_with(".rs"));
		assert!(
			repository.join(migration.verification).is_file(),
			"missing migration verification {}",
			migration.verification
		);
	}
}
