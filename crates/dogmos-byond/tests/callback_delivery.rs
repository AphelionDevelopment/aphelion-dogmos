use std::{fs, path::Path};

#[test]
fn generated_bindings_include_production_callback_and_continuation_exports() {
	let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
	let source = fs::read_to_string(crate_root.join("src/lib.rs")).unwrap();
	let bindings = fs::read_to_string(crate_root.join("bindings.dm")).unwrap();
	for binding in [
		"/proc/dogmos_callback_drain",
		"/proc/dogmos_continuation_adjust_multiple",
		"/proc/dogmos_continuation_cancel",
		"/proc/dogmos_continuation_command",
		"/proc/dogmos_continuation_resume",
	] {
		assert!(
			source.contains(&format!("#[auxmacros::bind(\"{binding}\")]")),
			"missing production callback export {binding}"
		);
		assert!(
			bindings.contains(&format!("{binding}(")),
			"generated bindings omit production callback export {binding}"
		);
	}
}
