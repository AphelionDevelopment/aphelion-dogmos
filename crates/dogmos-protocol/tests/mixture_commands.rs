use dogmos_protocol::{
	decode_adjust_multiple_request, encode_adjust_multiple_request, MixtureAdjustment,
	MixtureCommandRequest, MixtureCommandResponse, OperationKind, ProtocolError, ScalarValue,
	WireHandle, MIXTURE_ADJUSTMENT_LEN, MIXTURE_ADJUST_MULTIPLE_HEADER_LEN,
	MIXTURE_COMMAND_REQUEST_LEN, MIXTURE_COMMAND_RESPONSE_LEN,
};

fn handle(slot: u32, generation: u32) -> WireHandle {
	WireHandle { slot, generation }
}

#[test]
fn mixture_command_operation_and_layout_are_stable() {
	assert_eq!(OperationKind::MixtureCommand as u16, 28);
	assert_eq!(OperationKind::MixtureAdjustMultiple as u16, 31);
	assert_eq!(MIXTURE_COMMAND_REQUEST_LEN, 56);
	assert_eq!(MIXTURE_COMMAND_RESPONSE_LEN, 24);
	assert_eq!(MIXTURE_ADJUST_MULTIPLE_HEADER_LEN, 12);
	assert_eq!(MIXTURE_ADJUSTMENT_LEN, 16);
	let request = MixtureCommandRequest::SetMoles {
		handle: handle(1, 2),
		gas_id: 3,
		amount: ScalarValue(4.5),
	};
	let bytes = request.encode().unwrap();
	assert_eq!(&bytes[0..2], &1_u16.to_le_bytes());
	assert_eq!(&bytes[4..12], &handle(1, 2).encode());
	assert_eq!(&bytes[20..28], &4.5_f64.to_le_bytes());
	assert_eq!(&bytes[44..46], &3_u16.to_le_bytes());
	assert_eq!(MixtureCommandRequest::decode(&bytes), Ok(request));
}

#[test]
fn adjust_multiple_uses_bounded_fixed_records_and_rejects_padding() {
	let mixture = handle(7, 8);
	let adjustments = [
		MixtureAdjustment {
			gas_id: 1,
			delta: ScalarValue(2.5),
		},
		MixtureAdjustment {
			gas_id: 3,
			delta: ScalarValue(-0.5),
		},
	];
	let mut bytes = Vec::new();
	encode_adjust_multiple_request(mixture, &adjustments, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 12 + 2 * 16);
	assert_eq!(
		decode_adjust_multiple_request(&bytes),
		Ok((mixture, adjustments.to_vec()))
	);

	bytes[12 + 2] = 1;
	assert_eq!(
		decode_adjust_multiple_request(&bytes),
		Err(ProtocolError::ReservedMixtureAdjustmentField)
	);
}

#[test]
fn every_fixed_mixture_command_round_trips() {
	let first = handle(1, 2);
	let second = handle(3, 4);
	let commands = vec![
		MixtureCommandRequest::SetMoles {
			handle: first,
			gas_id: 5,
			amount: ScalarValue(6.0),
		},
		MixtureCommandRequest::AdjustMoles {
			handle: first,
			gas_id: 5,
			delta: ScalarValue(-1.0),
		},
		MixtureCommandRequest::AdjustMolesTemperature {
			handle: first,
			gas_id: 5,
			amount: ScalarValue(2.0),
			temperature: ScalarValue(500.0),
		},
		MixtureCommandRequest::GetMoles {
			handle: first,
			gas_id: 5,
		},
		MixtureCommandRequest::Temperature { handle: first },
		MixtureCommandRequest::Volume { handle: first },
		MixtureCommandRequest::HeatCapacity { handle: first },
		MixtureCommandRequest::PartialHeatCapacity {
			handle: first,
			gas_id: 5,
		},
		MixtureCommandRequest::TotalMoles { handle: first },
		MixtureCommandRequest::Pressure { handle: first },
		MixtureCommandRequest::ThermalEnergy { handle: first },
		MixtureCommandRequest::GetMolesByFlags {
			handle: first,
			flags: 0x55aa,
		},
		MixtureCommandRequest::Burnability {
			handle: first,
			temperature: Some(ScalarValue(400.0)),
		},
		MixtureCommandRequest::SetTemperature {
			handle: first,
			temperature: ScalarValue(400.0),
		},
		MixtureCommandRequest::SetVolume {
			handle: first,
			volume: ScalarValue(2500.0),
		},
		MixtureCommandRequest::SetMinimumHeatCapacity {
			handle: first,
			amount: ScalarValue(80.0),
		},
		MixtureCommandRequest::Clear { handle: first },
		MixtureCommandRequest::Add {
			handle: first,
			amount: ScalarValue(-1.0),
		},
		MixtureCommandRequest::Multiply {
			handle: first,
			factor: ScalarValue(0.5),
		},
		MixtureCommandRequest::CopyFrom {
			receiver: first,
			giver: second,
		},
		MixtureCommandRequest::AdjustHeat {
			handle: first,
			heat: ScalarValue(100.0),
		},
		MixtureCommandRequest::Compare {
			left: first,
			right: second,
		},
		MixtureCommandRequest::EqualizeWith {
			receiver: first,
			total: second,
		},
		MixtureCommandRequest::TemperatureShare {
			first,
			second,
			conduction_coefficient: ScalarValue(0.4),
		},
		MixtureCommandRequest::TemperatureShareNonGas {
			handle: first,
			conduction_coefficient: ScalarValue(0.4),
			sharer_temperature: ScalarValue(300.0),
			sharer_heat_capacity: ScalarValue(100.0),
		},
		MixtureCommandRequest::MarkImmutable { handle: first },
		MixtureCommandRequest::IsImmutable { handle: first },
		MixtureCommandRequest::Merge {
			receiver: first,
			giver: second,
		},
		MixtureCommandRequest::RemoveRatioInto {
			source: first,
			destination: second,
			ratio: ScalarValue(0.25),
		},
		MixtureCommandRequest::RemoveAmountInto {
			source: first,
			destination: second,
			amount: ScalarValue(5.0),
		},
		MixtureCommandRequest::TransferGases {
			source: first,
			destination: second,
			ratio: ScalarValue(0.25),
			gas_mask: 0x8000_0001,
		},
		MixtureCommandRequest::TransferAmount {
			source: first,
			destination: second,
			amount: ScalarValue(5.0),
		},
		MixtureCommandRequest::TransferRatio {
			source: first,
			destination: second,
			ratio: ScalarValue(0.25),
		},
		MixtureCommandRequest::TransferByFlags {
			source: first,
			destination: second,
			flags: 3,
			amount: ScalarValue(5.0),
		},
		MixtureCommandRequest::ShareRatio {
			first,
			second,
			ratio: ScalarValue(0.4),
			one_way: true,
		},
	];
	for (index, command) in commands.into_iter().enumerate() {
		let bytes = command.encode().unwrap();
		assert_eq!(&bytes[0..2], &(index as u16 + 1).to_le_bytes());
		assert_eq!(MixtureCommandRequest::decode(&bytes), Ok(command));
	}
}

#[test]
fn mixture_command_rejects_non_finite_and_unknown_discriminants() {
	let mut bytes = MixtureCommandRequest::SetTemperature {
		handle: handle(1, 2),
		temperature: ScalarValue(300.0),
	}
	.encode()
	.unwrap();
	bytes[20..28].copy_from_slice(&f64::NAN.to_le_bytes());
	assert_eq!(
		MixtureCommandRequest::decode(&bytes),
		Err(ProtocolError::NonFiniteScalar)
	);
	bytes[0..2].copy_from_slice(&u16::MAX.to_le_bytes());
	assert_eq!(
		MixtureCommandRequest::decode(&bytes),
		Err(ProtocolError::UnknownMixtureCommand(u16::MAX))
	);
}

#[test]
fn mixture_command_rejects_nonzero_unused_fields() {
	let canonical = MixtureCommandRequest::Temperature {
		handle: handle(1, 2),
	}
	.encode()
	.unwrap();
	for (offset, field) in [
		(12, handle(3, 4).encode().to_vec()),
		(20, 1.0_f64.to_le_bytes().to_vec()),
		(28, 1.0_f64.to_le_bytes().to_vec()),
		(36, 1.0_f64.to_le_bytes().to_vec()),
		(44, 1_u16.to_le_bytes().to_vec()),
		(48, 1_u32.to_le_bytes().to_vec()),
	] {
		let mut payload = canonical;
		payload[offset..offset + field.len()].copy_from_slice(&field);
		assert_eq!(
			MixtureCommandRequest::decode(&payload),
			Err(ProtocolError::ReservedMixtureCommandField)
		);
	}

	let mut absent_burnability = MixtureCommandRequest::Burnability {
		handle: handle(1, 2),
		temperature: None,
	}
	.encode()
	.unwrap();
	absent_burnability[20..28].copy_from_slice(&300.0_f64.to_le_bytes());
	assert_eq!(
		MixtureCommandRequest::decode(&absent_burnability),
		Err(ProtocolError::ReservedMixtureCommandField)
	);
}

#[test]
fn mixture_command_responses_have_one_fixed_layout() {
	let responses = [
		MixtureCommandResponse::Applied { updated: 2 },
		MixtureCommandResponse::Scalar(ScalarValue(4.5)),
		MixtureCommandResponse::Scalars([ScalarValue(1.0), ScalarValue(2.0)]),
		MixtureCommandResponse::Boolean(true),
	];
	for (index, response) in responses.into_iter().enumerate() {
		let bytes = response.encode().unwrap();
		assert_eq!(&bytes[0..4], &(index as u32 + 1).to_le_bytes());
		assert_eq!(MixtureCommandResponse::decode(&bytes), Ok(response));
	}
}
