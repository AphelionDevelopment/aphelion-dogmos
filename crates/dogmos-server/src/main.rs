fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
	let mut arguments = std::env::args().skip(1);
	match (
		arguments.next().as_deref(),
		arguments.next(),
		arguments.next(),
	) {
		(Some("--echo-server"), Some(endpoint), None) => dogmos_server::run(&endpoint),
		_ => Err("usage: dogmosd --echo-server <endpoint>".into()),
	}
}
