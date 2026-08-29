use dogmos_protocol::{
	CallbackBatchHeader, CallbackBatchRequest, CallbackEvent, CallbackEventKind, CallbackScope,
	ContinuationToken, ProtocolError, ReactionKind, ScalarValue, TurfDestructionReason, WireHandle,
	CALLBACK_BATCH_HEADER_LEN, CALLBACK_BATCH_REQUEST_LEN, CALLBACK_EVENT_LEN,
};

fn continuation() -> ContinuationToken {
	ContinuationToken {
		world_generation: 7,
		id: 11,
		deadline_ticks: 50,
	}
}

#[test]
fn callback_event_has_a_strict_cross_bitness_layout() {
	let event = CallbackEvent {
		scope_sequence: 0x0102_0304_0506_0708,
		transaction_id: 0,
		scope: CallbackScope::General,
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
		continuation: None,
	};
	let encoded = event.encode().unwrap();

	assert_eq!(encoded.len(), CALLBACK_EVENT_LEN);
	assert_eq!(&encoded[0..8], &event.scope_sequence.to_le_bytes());
	assert_eq!(&encoded[8..16], &event.transaction_id.to_le_bytes());
	assert_eq!(&encoded[16..18], &(event.scope as u16).to_le_bytes());
	assert_eq!(&encoded[18..20], &(event.kind as u16).to_le_bytes());
	assert_eq!(&encoded[20..22], &event.flags.to_le_bytes());
	assert_eq!(&encoded[22..24], &[0; 2]);
	assert_eq!(&encoded[24..32], &event.subject.encode());
	assert_eq!(&encoded[32..40], &event.target.encode());
	assert_eq!(&encoded[40..48], &123.5_f64.to_le_bytes());
	assert_eq!(&encoded[48..56], &(-45.25_f64).to_le_bytes());
	assert_eq!(&encoded[56..64], &0.125_f64.to_le_bytes());
	assert_eq!(&encoded[64..72], &1_000_000_f64.to_le_bytes());
	assert_eq!(&encoded[72..76], &event.aux.to_le_bytes());
	assert_eq!(&encoded[76..80], &[0; 4]);
	assert_eq!(&encoded[80..104], &[0; 24]);
	assert_eq!(CallbackEvent::decode(&encoded).unwrap(), event);
}

#[test]
fn every_implemented_gameplay_event_kind_round_trips() {
	for (index, kind) in [
		CallbackEventKind::Diagnostic,
		CallbackEventKind::ReactionFinished,
		CallbackEventKind::PressureDifference,
		CallbackEventKind::DecompressionFloorRip,
		CallbackEventKind::FirelockConsideration,
		CallbackEventKind::TurfDestructionRequest,
		CallbackEventKind::RunDmReaction,
		CallbackEventKind::ReactionProfiled,
	]
	.into_iter()
	.enumerate()
	{
		let aux = match kind {
			CallbackEventKind::ReactionFinished => ReactionKind::Plasma as u32,
			CallbackEventKind::TurfDestructionRequest => {
				TurfDestructionReason::SuperconductiveHeat as u32
			}
			CallbackEventKind::RunDmReaction => 37,
			CallbackEventKind::ReactionProfiled => 37,
			_ => 0,
		};
		let event = CallbackEvent {
			scope_sequence: 1,
			transaction_id: 0,
			scope: CallbackScope::General,
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
			continuation: (kind == CallbackEventKind::RunDmReaction).then(continuation),
		};
		let bytes = event.encode().unwrap();
		assert_eq!(&bytes[18..20], &(index as u16 + 1).to_le_bytes());
		assert_eq!(CallbackEvent::decode(&bytes).unwrap(), event);
	}
}

#[test]
fn dm_reaction_request_has_a_fixed_width_golden_layout() {
	let event = CallbackEvent {
		scope_sequence: 9,
		transaction_id: 31,
		scope: CallbackScope::Reaction,
		kind: CallbackEventKind::RunDmReaction,
		flags: 0,
		subject: WireHandle {
			slot: 7,
			generation: 3,
		},
		target: WireHandle {
			slot: 11,
			generation: 5,
		},
		values: [
			ScalarValue(0.0),
			ScalarValue(0.0),
			ScalarValue(0.0),
			ScalarValue(0.0),
		],
		aux: 19,
		continuation: Some(continuation()),
	};
	let encoded = event.encode().unwrap();
	assert_eq!(encoded.len(), CALLBACK_EVENT_LEN);
	assert_eq!(&encoded[8..16], &31_u64.to_le_bytes());
	assert_eq!(
		&encoded[16..18],
		&(CallbackScope::Reaction as u16).to_le_bytes()
	);
	assert_eq!(&encoded[18..20], &7_u16.to_le_bytes());
	assert_eq!(&encoded[24..32], &event.subject.encode());
	assert_eq!(&encoded[32..40], &event.target.encode());
	assert_eq!(&encoded[72..76], &19_u32.to_le_bytes());
	assert_eq!(&encoded[80..84], &7_u32.to_le_bytes());
	assert_eq!(&encoded[88..96], &11_u64.to_le_bytes());
	assert_eq!(&encoded[96..104], &50_u64.to_le_bytes());
	assert_eq!(CallbackEvent::decode(&encoded).unwrap(), event);
}

#[test]
fn callback_event_rejects_reserved_flags_unknown_kinds_and_nonfinite_values() {
	let mut encoded = CallbackEvent {
		scope_sequence: 1,
		transaction_id: 0,
		scope: CallbackScope::General,
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
		continuation: None,
	}
	.encode()
	.unwrap();

	encoded[18..20].copy_from_slice(&99_u16.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackEventKind(99))
	));

	encoded[18..20].copy_from_slice(&(CallbackEventKind::Diagnostic as u16).to_le_bytes());
	encoded[20..22].copy_from_slice(&1_u16.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackFlags(1))
	));

	encoded[20..22].copy_from_slice(&0_u16.to_le_bytes());
	encoded[40..48].copy_from_slice(&f64::NAN.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::NonFiniteScalar)
	));

	encoded[40..48].copy_from_slice(&5.0_f64.to_le_bytes());
	encoded[64..72].copy_from_slice(&f64::INFINITY.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::NonFiniteScalar)
	));

	encoded[64..72].copy_from_slice(&5.0_f64.to_le_bytes());
	encoded[72..76].copy_from_slice(&1_u32.to_le_bytes());
	assert!(matches!(
		CallbackEvent::decode(&encoded),
		Err(ProtocolError::UnknownCallbackAux { kind: 1, actual: 1 })
	));
}

#[test]
fn callback_event_requires_continuations_only_for_dm_reactions() {
	let dm_reaction = CallbackEvent {
		scope_sequence: 1,
		transaction_id: 9,
		scope: CallbackScope::Reaction,
		kind: CallbackEventKind::RunDmReaction,
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
		aux: 5,
		continuation: Some(continuation()),
	};
	let mut missing = dm_reaction.encode().unwrap();
	missing[80..104].fill(0);
	assert_eq!(
		CallbackEvent::decode(&missing),
		Err(ProtocolError::MissingContinuationToken)
	);

	let mut unexpected = CallbackEvent {
		kind: CallbackEventKind::Diagnostic,
		aux: 0,
		continuation: None,
		..dm_reaction
	}
	.encode()
	.unwrap();
	unexpected[80..104].copy_from_slice(&continuation().encode().unwrap());
	assert_eq!(
		CallbackEvent::decode(&unexpected),
		Err(ProtocolError::UnexpectedContinuationToken)
	);
}

#[test]
fn callback_batch_request_and_header_have_strict_lengths() {
	let request = CallbackBatchRequest {
		max_events: 256,
		scope: CallbackScope::Reaction,
		transaction_id: 0x0102_0304_0506_0708,
	};
	let request_bytes = request.encode().unwrap();
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

#[test]
fn callback_scope_and_transaction_pairing_is_strict() {
	let general = CallbackBatchRequest {
		max_events: 1,
		scope: CallbackScope::General,
		transaction_id: 1,
	};
	assert_eq!(
		general.encode(),
		Err(ProtocolError::InvalidCallbackTransaction {
			scope: CallbackScope::General as u16,
			transaction_id: 1,
		})
	);

	let reaction = CallbackBatchRequest {
		max_events: 1,
		scope: CallbackScope::Reaction,
		transaction_id: 0,
	};
	assert_eq!(
		reaction.encode(),
		Err(ProtocolError::InvalidCallbackTransaction {
			scope: CallbackScope::Reaction as u16,
			transaction_id: 0,
		})
	);
}

#[test]
fn callback_decoders_reject_unknown_scopes_and_reserved_fields() {
	let request = CallbackBatchRequest {
		max_events: 1,
		scope: CallbackScope::General,
		transaction_id: 0,
	}
	.encode()
	.unwrap();
	let mut unknown_scope = request;
	unknown_scope[4..6].copy_from_slice(&99_u16.to_le_bytes());
	assert_eq!(
		CallbackBatchRequest::decode(&unknown_scope),
		Err(ProtocolError::UnknownCallbackScope(99))
	);
	let mut reserved_request = request;
	reserved_request[6..8].copy_from_slice(&5_u16.to_le_bytes());
	assert_eq!(
		CallbackBatchRequest::decode(&reserved_request),
		Err(ProtocolError::ReservedCallbackBatchField(5))
	);

	let event = CallbackEvent {
		scope_sequence: 1,
		transaction_id: 0,
		scope: CallbackScope::General,
		kind: CallbackEventKind::Diagnostic,
		flags: 0,
		subject: WireHandle {
			slot: 1,
			generation: 2,
		},
		target: WireHandle {
			slot: 0,
			generation: 0,
		},
		values: [ScalarValue(0.0); 4],
		aux: 0,
		continuation: None,
	}
	.encode()
	.unwrap();
	let mut reserved_event = event;
	reserved_event[76..80].copy_from_slice(&9_u32.to_le_bytes());
	assert_eq!(
		CallbackEvent::decode(&reserved_event),
		Err(ProtocolError::ReservedCallbackEventField(9))
	);
}
