use dogmos_core::metadata::{GasFireRole, GasId, GasMetadata};
use dogmos_core::world::{
	Command, DogmosWorld, LifecycleAction, LifecycleMutation, MixtureSnapshot,
};
use dogmos_core::MixtureHandle;

const LEGACY_TRANSCRIPT: &str = include_str!("fixtures/legacy_mixture_transcript_v1.txt");
const TRANSCRIPT_HEADER: &str = "DOGMOS_LEGACY_MIXTURE_TRANSCRIPT_V1";
const EXPECTED_STEPS: [(&str, &str, f32); 12] = [
	("set_o2", "null", 0.0),
	("set_n2", "null", 0.0),
	("adjust_o2", "null", 0.0),
	("set_temperature", "null", 0.0),
	("set_volume", "null", 0.0),
	("seed_b", "null", 0.0),
	("temperature_b", "null", 0.0),
	("merge", "number", 1.0),
	("remove_ratio", "mixture", 1.0),
	("transfer_amount", "null", 0.0),
	("equalize", "null", 0.0),
	("immutable_write", "null", 0.0),
];

#[derive(Debug)]
struct CapturedStep {
	name: String,
	result_kind: String,
	result_value: f32,
	mixtures: [[f32; 4]; 3],
}

fn gas(id: u16, key: &str, specific_heat: f32) -> GasMetadata {
	GasMetadata {
		id: GasId(id),
		key: key.into(),
		name: key.into(),
		flags: 0,
		specific_heat,
		fusion_power: 0.0,
		moles_visible: None,
		enthalpy: 0.0,
		fire_radiation_released: 0.0,
		fire_role: GasFireRole::None,
		fire_products: None,
	}
}

fn handle(slot: u32) -> MixtureHandle {
	MixtureHandle {
		slot,
		generation: 1,
	}
}

fn parse_transcript() -> Vec<CapturedStep> {
	let mut lines = LEGACY_TRANSCRIPT.lines();
	assert_eq!(lines.next(), Some(TRANSCRIPT_HEADER));
	lines
		.map(|line| {
			let fields = line.split('|').collect::<Vec<_>>();
			assert_eq!(fields.len(), 15, "malformed transcript row: {line}");
			let mut values = [0.0_f32; 12];
			for (index, value) in fields[3..].iter().enumerate() {
				values[index] = value
					.parse::<f32>()
					.unwrap_or_else(|_| panic!("invalid transcript scalar {value} in {line}"));
			}
			CapturedStep {
				name: fields[0].to_owned(),
				result_kind: fields[1].to_owned(),
				result_value: fields[2]
					.parse::<f32>()
					.unwrap_or_else(|_| panic!("invalid result scalar {} in {line}", fields[2])),
				mixtures: [
					values[0..4].try_into().unwrap(),
					values[4..8].try_into().unwrap(),
					values[8..12].try_into().unwrap(),
				],
			}
		})
		.collect()
}

fn snapshot_values(snapshot: &MixtureSnapshot) -> [f32; 4] {
	[
		snapshot.temperature,
		snapshot.volume,
		snapshot.gases[0],
		snapshot.gases[1],
	]
}

fn assert_close(actual: f32, expected: f32, step: &str, field: &str) {
	let tolerance = 0.000_1_f32.max(expected.abs() * 0.000_01);
	assert!(
		(actual - expected).abs() <= tolerance,
		"{step} {field}: expected {expected}, got {actual}, tolerance {tolerance}"
	);
}

fn assert_step(world: &DogmosWorld, captured: &CapturedStep) {
	for (slot, expected) in captured.mixtures.iter().enumerate() {
		let actual = snapshot_values(&world.snapshot(handle(slot as u32)).unwrap());
		for (field, (actual, expected)) in ["temperature", "volume", "o2", "n2"]
			.into_iter()
			.zip(actual.into_iter().zip(expected.iter().copied()))
		{
			assert_close(actual, expected, &captured.name, field);
		}
	}
}

#[test]
fn captured_legacy_mixture_transcript_replays_against_process_neutral_core() {
	let captured = parse_transcript();
	assert_eq!(captured.len(), EXPECTED_STEPS.len());
	for (captured, (name, result_kind, result_value)) in captured.iter().zip(EXPECTED_STEPS) {
		assert_eq!(captured.name, name);
		assert_eq!(captured.result_kind, result_kind);
		assert_close(captured.result_value, result_value, name, "result");
	}

	let mut world = DogmosWorld::new(1024 * 1024);
	world
		.install_gases(vec![gas(0, "o2", 20.0), gas(1, "n2", 20.0)])
		.unwrap();
	world
		.apply_lifecycle(
			&(0..4)
				.map(|slot| LifecycleMutation {
					action: LifecycleAction::Register,
					handle: handle(slot),
				})
				.collect::<Vec<_>>(),
		)
		.unwrap();

	for step in &captured {
		match step.name.as_str() {
			"set_o2" => world
				.apply_command(Command::SetMoles {
					handle: handle(0),
					gas: GasId(0),
					amount: 100.0,
				})
				.unwrap(),
			"set_n2" => world
				.apply_command(Command::SetMoles {
					handle: handle(0),
					gas: GasId(1),
					amount: 50.0,
				})
				.unwrap(),
			"adjust_o2" => world
				.apply_command(Command::AdjustMoles {
					handle: handle(0),
					gas: GasId(0),
					delta: -25.0,
				})
				.unwrap(),
			"set_temperature" => world
				.apply_command(Command::SetTemperature {
					handle: handle(0),
					temperature: 400.0,
				})
				.unwrap(),
			"set_volume" => world
				.apply_command(Command::SetVolume {
					handle: handle(0),
					volume: 2000.0,
				})
				.unwrap(),
			"seed_b" => world
				.apply_command(Command::SetMoles {
					handle: handle(1),
					gas: GasId(0),
					amount: 20.0,
				})
				.unwrap(),
			"temperature_b" => world
				.apply_command(Command::SetTemperature {
					handle: handle(1),
					temperature: 300.0,
				})
				.unwrap(),
			"merge" => world
				.apply_command(Command::Merge {
					receiver: handle(0),
					giver: handle(1),
				})
				.unwrap(),
			"remove_ratio" => {
				let source_volume = world.snapshot(handle(0)).unwrap().volume;
				world
					.apply_command(Command::SetVolume {
						handle: handle(2),
						volume: source_volume,
					})
					.unwrap();
				world
					.apply_command(Command::RemoveRatioInto {
						source: handle(0),
						destination: handle(2),
						ratio: 0.25,
					})
					.unwrap()
			}
			"transfer_amount" => world
				.apply_command(Command::TransferAmount {
					source: handle(0),
					destination: handle(1),
					amount: 10.0,
				})
				.unwrap(),
			"equalize" => {
				let total_volume = world.snapshot(handle(0)).unwrap().volume
					+ world.snapshot(handle(1)).unwrap().volume;
				world
					.apply_command(Command::Clear { handle: handle(3) })
					.unwrap();
				world
					.apply_command(Command::SetVolume {
						handle: handle(3),
						volume: total_volume,
					})
					.unwrap();
				for giver in [handle(0), handle(1)] {
					world
						.apply_command(Command::Merge {
							receiver: handle(3),
							giver,
						})
						.unwrap();
				}
				let mut result = None;
				for receiver in [handle(0), handle(1)] {
					result = Some(
						world
							.apply_command(Command::EqualizeWith {
								receiver,
								total: handle(3),
							})
							.unwrap(),
					);
				}
				result.unwrap()
			}
			"immutable_write" => {
				world
					.apply_command(Command::MarkImmutable { handle: handle(2) })
					.unwrap();
				world
					.apply_command(Command::SetMoles {
						handle: handle(2),
						gas: GasId(0),
						amount: 999.0,
					})
					.unwrap()
			}
			other => panic!("unknown captured step {other}"),
		};
		assert_step(&world, step);
	}
}
