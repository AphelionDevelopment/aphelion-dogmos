use dogmos_protocol::{
	CallbackBatchHeader, CallbackBatchRequest, CallbackEvent, CallbackEventKind, ProtocolError,
	ReactionKind, ScalarValue, TurfDestructionReason, WireHandle, CALLBACK_BATCH_HEADER_LEN,
	CALLBACK_BATCH_REQUEST_LEN, CALLBACK_EVENT_LEN,
};

#[test]
fn callback_event_has_a_strict_cross_bitness_layout() {
	let event = CallbackEvent {
		sequence: 0x0102_0304_0506_0708,
		kind: CallbackEventKind::Diagnostic,
		flags: 0,
		subject: WireHandle {
			slot: 0x1112_1314,
			generation: 0x2122_2324,
		},
		target: WireHandle {
			slot: 0x3132_3334,
			generation: 0x4142_4344,
		},
		values: [
			ScalarValue(123.5),
			ScalarValue(-45.25),
			ScalarValue(0.125),
			ScalarValue(1_000_000.0),
		],
		aux: 0,
	};
	let encoded = event.encode().unwrap();

	assert_eq!(encoded.len(), CALLBACK_EVENT_LEN);
	assert_eq!(&encoded[0..8], &event.sequence.to_le_bytes());
	assert_eq!(&encoded[8..10], &(event.kind as u16).to_le_bytes());
	assert_eq!(&encoded[10..12], &event.flags.to_le_bytes());
	assert_eq!(&encoded[12..20], &event.subject.encode());
	assert_eq!(&encoded[20..28], &event.target.encode());
	assert_eq!(&encoded[28..36], &123.5_f64.to_le_bytes());
	assert_eq!(&encoded[36..44], &(-45.25_f64).to_le_bytes());
	assert_eq!(&encoded[44..52], &0.125_f64.to_le_bytes());
	assert_eq!(&encoded[52..60], &1_000_000_f64.to_le_bytes());
	assert_eq!(&encoded[60..64], &event.aux.to_le_bytes());
	assert_eq!(CallbackEvent::decode(&encoded).unwrap(), event);
}

#[test]
fn every_implemented_gameplay_event_kind_round_trips() {
	for kind in [
		CallbackEventKind::Diagnostic,
		CallbackEventKind::ReactionFinished,
		CallbackEventKind::PressureDifference,
		CallbackEventKind::DecompressionFloorRip,
		CallbackEventKind::FirelockConsideration,
		CallbackEventKind::TurfDestructionRequest,
	] {
		let aux = match kind {
			CallbackEventKind::ReactionFinished => ReactionKind::Plasma as u32,
			CallbackEventKind::TurfDestructionRequest => {
				TurfDestructionReason::SuperconductiveHeat as u32
			}
			_ => 0,
		};
		let event = CallbackEvent {
			sequence: 1,
			kind,
			flags: 0,
			subject: WireHandle {
				slot: 1,
				generation: 2,
			},
			target: WireHandle {
				slot: 3,
				generation: 4,
			},
			values: [ScalarValue(0.0); 4],
			aux,
		};
		assert_eq!(
			CallbackEvent::decode(&event.encode().unwrap()).unwrap(),
			event
		);
	}
}

#[test]
fn callback_event_rejects_reserved_flags_unknown_kinds_and_nonfinite_values() {
	let mut encoded = CallbackEvent {
		sequence: 1,
		kind: CallbackEventKind::Diagnostic,
		flags: 0,
		subject: WireHandle {
			slot: 1,
			generation: 2,
		},
		target: WireHandle {
			slot: 3,
			generation: 4,
		},
		values: [ScalarValue(5.0); 4],
		aux: 0,
	}
	.encode()
	.unwrap();

	encoded[8..10].copy_from_slice(&99_u16.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackEventKind(99))
	));

	encoded[8..10].copy_from_slice(&(CallbackEventKind::Diagnostic as u16).to_le_bytes());
	encoded[10..12].copy_from_slice(&1_u16.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackFlags(1))
	));

	encoded[10..12].copy_from_slice(&0_u16.to_le_bytes());
	encoded[28..36].copy_from_slice(&f64::NAN.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::NonFiniteScalar)
	));

	encoded[28..36].copy_from_slice(&5.0_f64.to_le_bytes());
	encoded[52..60].copy_from_slice(&f64::INFINITY.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::NonFiniteScalar)
	));

	encoded[52..60].copy_from_slice(&5.0_f64.to_le_bytes());
	encoded[60..64].copy_from_slice(&1_u32.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackAux { kind: 1, actual: 1 })
	));
}

#[test]
fn callback_batch_request_and_header_have_strict_lengths() {
	let request = CallbackBatchRequest { max_events: 256 };
	let request_bytes = request.encode();
	assert_eq!(request_bytes.len(), CALLBACK_BATCH_REQUEST_LEN);
	assert_eq!(
		CallbackBatchRequest::decode(&request_bytes).unwrap(),
		request
	);
	assert!(CallbackBatchRequest::decode(&[0; 3]).is_err());

	let header = CallbackBatchHeader {
		returned: 4,
		remaining: 12,
		capacity: 1024,
		high_water: 900,
		rejected: 33,
	};
	let header_bytes = header.encode();
	assert_eq!(header_bytes.len(), CALLBACK_BATCH_HEADER_LEN);
	assert_eq!(CallbackBatchHeader::decode(&header_bytes).unwrap(), header);
	assert!(CallbackBatchHeader::decode(&[0; 23]).is_err());
}
