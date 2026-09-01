use dogmos_protocol::{
	decode_adjacency_batch, decode_frontier_append_into, decode_frontier_mutate_into,
	decode_lifecycle_batch, decode_mixture_state_batch, decode_pipenet_reconcile_request,
	decode_pipenet_reconcile_response, encode_mixture_state_batch,
	encode_pipenet_reconcile_request, encode_pipenet_reconcile_response, AdjacencyMutation,
	FrontierAppendRequest, FrontierAppendResponse, FrontierBeginRequest, FrontierBeginResponse,
	FrontierCommitRequest, FrontierCommitResponse, FrontierMutateRequest, FrontierMutateResponse,
	LifecycleAction, LifecycleMutation, MixtureCommandResponse, MixtureSnapshot,
	MixtureSnapshotRequest, MixtureStateMutation, MixtureStateUploadAbortRequest,
	MixtureStateUploadAppendRequest, MixtureStateUploadBeginRequest,
	MixtureStateUploadCommitRequest, OperationKind, PipenetReconcileSnapshot, ProtocolError,
	ScalarValue, SimulationStage, SimulationStageRequest, SimulationStageResponse, WireHandle,
	ADJACENCY_MUTATION_LEN, DOGMOS_PROTOCOL_VERSION, FRONTIER_APPEND_HEADER_LEN,
	FRONTIER_APPEND_RESPONSE_LEN, FRONTIER_BEGIN_REQUEST_LEN, FRONTIER_BEGIN_RESPONSE_LEN,
	FRONTIER_COMMIT_REQUEST_LEN, FRONTIER_COMMIT_RESPONSE_LEN, FRONTIER_MUTATE_HEADER_LEN,
	FRONTIER_MUTATE_RESPONSE_LEN, LIFECYCLE_MUTATION_LEN, MAX_FRONTIER_APPEND_HANDLES,
	MAX_GAS_SLOTS, MIXTURE_SNAPSHOT_LEN, MIXTURE_STATE_MUTATION_LEN,
	PIPENET_RECONCILE_SNAPSHOT_LEN, SIMULATION_STAGE_REQUEST_LEN, SIMULATION_STAGE_RESPONSE_LEN,
};

fn handle(slot: u32, generation: u32) -> WireHandle {
	WireHandle { slot, generation }
}

fn decode_hex_fixture(input: &str) -> Vec<u8> {
	let digits = input
		.bytes()
		.filter(|byte| !byte.is_ascii_whitespace())
		.collect::<Vec<_>>();
	let (pairs, remainder) = digits.as_chunks::<2>();
	assert!(remainder.is_empty());
	pairs
		.iter()
		.map(|pair| {
			let digit = |value| match value {
				b'0'..=b'9' => value - b'0',
				b'a'..=b'f' => value - b'a' + 10,
				_ => panic!("invalid hex fixture"),
			};
			(digit(pair[0]) << 4) | digit(pair[1])
		})
		.collect()
}

#[test]
fn compound_operation_ids_are_stable() {
	assert_eq!(DOGMOS_PROTOCOL_VERSION, 12);
	assert_eq!(OperationKind::MixtureSnapshot as u16, 18);
	assert_eq!(OperationKind::MixtureLifecycleBatch as u16, 19);
	assert_eq!(OperationKind::AdjacencyBatch as u16, 20);
	assert_eq!(OperationKind::SimulationStage as u16, 21);
	assert_eq!(OperationKind::MixtureStateBatch as u16, 23);
	assert_eq!(OperationKind::TurfLifecycleBatch as u16, 24);
	assert_eq!(OperationKind::TurfAdjacencyBatch as u16, 25);
	assert_eq!(OperationKind::TurfHeatBatch as u16, 26);
	assert_eq!(OperationKind::TurfHeatAdjacencyBatch as u16, 27);
	assert_eq!(OperationKind::TurfHeatSnapshot as u16, 37);
	assert_eq!(OperationKind::FrontierBegin as u16, 38);
	assert_eq!(OperationKind::FrontierAppend as u16, 39);
	assert_eq!(OperationKind::FrontierCommit as u16, 40);
	assert_eq!(OperationKind::FrontierAdd as u16, 41);
	assert_eq!(OperationKind::FrontierRemove as u16, 42);
	assert_eq!(OperationKind::MixtureStateUploadBegin as u16, 43);
	assert_eq!(OperationKind::MixtureStateUploadAppend as u16, 44);
	assert_eq!(OperationKind::MixtureStateUploadCommit as u16, 45);
	assert_eq!(OperationKind::MixtureStateUploadAbort as u16, 46);
	assert_eq!(OperationKind::PipenetReconcile as u16, 47);
	assert_eq!(OperationKind::MixtureCommand as u16, 28);
	assert_eq!(OperationKind::GasMetadataInstall as u16, 29);
	assert_eq!(OperationKind::ReactionMetadataInstall as u16, 30);
	assert_eq!(OperationKind::MixtureAdjustMultiple as u16, 31);
	assert_eq!(
		OperationKind::try_from(18),
		Ok(OperationKind::MixtureSnapshot)
	);
	assert_eq!(
		OperationKind::try_from(21),
		Ok(OperationKind::SimulationStage)
	);
}

#[test]
fn pipenet_reconcile_request_has_an_exact_counted_handle_layout() {
	let handles = [handle(7, 11), handle(13, 17)];
	let mut encoded = Vec::new();
	encode_pipenet_reconcile_request(&handles, &mut encoded).unwrap();
	assert_eq!(
		encoded,
		decode_hex_fixture(
			"02 00 00 00
			 07 00 00 00 0b 00 00 00
			 0d 00 00 00 11 00 00 00"
		)
	);
	assert_eq!(
		decode_pipenet_reconcile_request(&encoded, 2).unwrap(),
		handles
	);
	assert_eq!(
		decode_pipenet_reconcile_request(&encoded, 1),
		Err(ProtocolError::OperationCountExceeded {
			actual: 2,
			maximum: 1,
		})
	);
}

#[test]
fn pipenet_reconcile_response_round_trips_handles_and_fixed_snapshots() {
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(12.5);
	let entry = PipenetReconcileSnapshot {
		handle: handle(7, 11),
		snapshot: MixtureSnapshot {
			revision: 19,
			gas_count: 1,
			temperature: ScalarValue(f64::from(293.15_f32)),
			volume: ScalarValue(2500.0),
			minimum_heat_capacity: ScalarValue(0.0),
			total_moles: ScalarValue(12.5),
			pressure: ScalarValue(f64::from(12.171_243_75_f32)),
			heat_capacity: ScalarValue(250.0),
			immutable: false,
			gases,
		},
	};
	let mut encoded = Vec::new();
	encode_pipenet_reconcile_response(&[entry], &mut encoded).unwrap();
	assert_eq!(encoded.len(), 4 + PIPENET_RECONCILE_SNAPSHOT_LEN);
	assert_eq!(&encoded[0..4], &1_u32.to_le_bytes());
	assert_eq!(&encoded[4..12], &entry.handle.encode());
	assert_eq!(
		decode_pipenet_reconcile_response(&encoded, 1).unwrap(),
		[entry]
	);
}

#[test]
fn oversized_dm_pipenet_response_fits_the_negotiated_control_buffer() {
	const OVERSIZED_DM_PIPELINE_MIXTURES: usize = 228;
	const PRODUCTION_CONTROL_PAYLOAD: usize = 64 * 1024;

	assert!(
		4 + OVERSIZED_DM_PIPELINE_MIXTURES * PIPENET_RECONCILE_SNAPSHOT_LEN
			<= PRODUCTION_CONTROL_PAYLOAD
	);
}

#[test]
fn frontier_requests_have_exact_bounded_layouts() {
	let begin = FrontierBeginRequest {
		epoch: 0x0102_0304_0506_0708,
		expected_count: 513,
	};
	let begin_bytes = begin.encode();
	assert_eq!(begin_bytes.len(), FRONTIER_BEGIN_REQUEST_LEN);
	assert_eq!(&begin_bytes[0..8], &begin.epoch.to_le_bytes());
	assert_eq!(&begin_bytes[8..12], &513_u32.to_le_bytes());
	assert_eq!(&begin_bytes[12..16], &[0; 4]);
	assert_eq!(FrontierBeginRequest::decode(&begin_bytes), Ok(begin));

	let handles = vec![handle(7, 11), handle(13, 17)];
	let append = FrontierAppendRequest {
		epoch: begin.epoch,
		offset: 511,
		handles: handles.clone(),
	};
	let append_bytes = append.encode().unwrap();
	assert_eq!(append_bytes.len(), FRONTIER_APPEND_HEADER_LEN + 16);
	assert_eq!(&append_bytes[0..8], &begin.epoch.to_le_bytes());
	assert_eq!(&append_bytes[8..12], &511_u32.to_le_bytes());
	assert_eq!(&append_bytes[12..16], &2_u32.to_le_bytes());
	assert_eq!(&append_bytes[16..24], &handles[0].encode());
	assert_eq!(&append_bytes[24..32], &handles[1].encode());
	let mut decoded_handles = vec![handle(99, 99)];
	let decoded_header = decode_frontier_append_into(&append_bytes, &mut decoded_handles).unwrap();
	assert_eq!(decoded_header.epoch, append.epoch);
	assert_eq!(decoded_header.offset, append.offset);
	assert_eq!(decoded_header.count, 2);
	assert_eq!(decoded_handles, handles);

	let commit = FrontierCommitRequest { epoch: begin.epoch };
	let commit_bytes = commit.encode();
	assert_eq!(commit_bytes.len(), FRONTIER_COMMIT_REQUEST_LEN);
	assert_eq!(FrontierCommitRequest::decode(&commit_bytes), Ok(commit));
}

#[test]
fn frontier_responses_have_exact_layouts() {
	let begin = FrontierBeginResponse { epoch: 41 };
	let begin_bytes = begin.encode();
	assert_eq!(begin_bytes.len(), FRONTIER_BEGIN_RESPONSE_LEN);
	assert_eq!(FrontierBeginResponse::decode(&begin_bytes), Ok(begin));
	let append = FrontierAppendResponse { accepted_count: 2 };
	let append_bytes = append.encode();
	assert_eq!(append_bytes.len(), FRONTIER_APPEND_RESPONSE_LEN);
	assert_eq!(FrontierAppendResponse::decode(&append_bytes), Ok(append));

	let commit = FrontierCommitResponse {
		epoch: 41,
		count: 513,
	};
	let commit_bytes = commit.encode();
	assert_eq!(commit_bytes.len(), FRONTIER_COMMIT_RESPONSE_LEN);
	assert_eq!(&commit_bytes[0..8], &41_u64.to_le_bytes());
	assert_eq!(&commit_bytes[8..12], &513_u32.to_le_bytes());
	assert_eq!(&commit_bytes[12..16], &[0; 4]);
	assert_eq!(FrontierCommitResponse::decode(&commit_bytes), Ok(commit));
}

#[test]
fn frontier_append_rejects_zero_oversized_duplicate_and_inexact_records() {
	let zero = FrontierAppendRequest {
		epoch: 1,
		offset: 0,
		handles: Vec::new(),
	};
	assert_eq!(
		zero.encode(),
		Err(ProtocolError::InvalidFrontierAppendCount(0))
	);

	let oversized = FrontierAppendRequest {
		epoch: 1,
		offset: 0,
		handles: vec![handle(1, 1); MAX_FRONTIER_APPEND_HANDLES + 1],
	};
	assert_eq!(
		oversized.encode(),
		Err(ProtocolError::InvalidFrontierAppendCount(
			(MAX_FRONTIER_APPEND_HANDLES + 1) as u32
		))
	);

	let duplicate = FrontierAppendRequest {
		epoch: 1,
		offset: 0,
		handles: vec![handle(1, 2), handle(1, 2)],
	};
	assert_eq!(
		duplicate.encode(),
		Err(ProtocolError::DuplicateFrontierHandle(handle(1, 2)))
	);

	let valid = FrontierAppendRequest {
		epoch: 1,
		offset: 0,
		handles: vec![handle(1, 2)],
	}
	.encode()
	.unwrap();
	assert!(matches!(
		decode_frontier_append_into(&valid[..valid.len() - 1], &mut Vec::new()),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
	let mut trailing = valid;
	trailing.push(0);
	assert!(matches!(
		decode_frontier_append_into(&trailing, &mut Vec::new()),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn frontier_mutate_layout_round_trips_and_rejects_zero_oversized_duplicate_and_inexact_records() {
	let handles = vec![handle(7, 11), handle(13, 17)];
	let add = FrontierMutateRequest {
		epoch: 0x0102_0304_0506_0708,
		handles: handles.clone(),
	};
	let add_bytes = add.encode().unwrap();
	assert_eq!(add_bytes.len(), FRONTIER_MUTATE_HEADER_LEN + 16);
	assert_eq!(&add_bytes[0..8], &add.epoch.to_le_bytes());
	assert_eq!(&add_bytes[8..12], &2_u32.to_le_bytes());
	assert_eq!(&add_bytes[12..20], &handles[0].encode());
	assert_eq!(&add_bytes[20..28], &handles[1].encode());
	let mut decoded_handles = vec![handle(99, 99)];
	let decoded_header = decode_frontier_mutate_into(&add_bytes, &mut decoded_handles).unwrap();
	assert_eq!(decoded_header.epoch, add.epoch);
	assert_eq!(decoded_header.count, 2);
	assert_eq!(decoded_handles, handles);

	let response = FrontierMutateResponse { count: 2 };
	let response_bytes = response.encode();
	assert_eq!(response_bytes.len(), FRONTIER_MUTATE_RESPONSE_LEN);
	assert_eq!(
		FrontierMutateResponse::decode(&response_bytes),
		Ok(response)
	);

	let zero = FrontierMutateRequest {
		epoch: 1,
		handles: Vec::new(),
	};
	assert_eq!(
		zero.encode(),
		Err(ProtocolError::InvalidFrontierAppendCount(0))
	);

	let oversized = FrontierMutateRequest {
		epoch: 1,
		handles: vec![handle(1, 1); MAX_FRONTIER_APPEND_HANDLES + 1],
	};
	assert_eq!(
		oversized.encode(),
		Err(ProtocolError::InvalidFrontierAppendCount(
			(MAX_FRONTIER_APPEND_HANDLES + 1) as u32
		))
	);

	let duplicate = FrontierMutateRequest {
		epoch: 1,
		handles: vec![handle(1, 2), handle(1, 2)],
	};
	assert_eq!(
		duplicate.encode(),
		Err(ProtocolError::DuplicateFrontierHandle(handle(1, 2)))
	);

	let valid = FrontierMutateRequest {
		epoch: 1,
		handles: vec![handle(1, 2)],
	}
	.encode()
	.unwrap();
	assert!(matches!(
		decode_frontier_mutate_into(&valid[..valid.len() - 1], &mut Vec::new()),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
	let mut trailing = valid;
	trailing.push(0);
	assert!(matches!(
		decode_frontier_mutate_into(&trailing, &mut Vec::new()),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn frontier_and_stage_reserved_fields_are_rejected() {
	let mut begin = FrontierBeginRequest {
		epoch: 1,
		expected_count: 1,
	}
	.encode();
	begin[12..16].copy_from_slice(&7_u32.to_le_bytes());
	assert_eq!(
		FrontierBeginRequest::decode(&begin),
		Err(ProtocolError::ReservedFrontierField(7))
	);

	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfs,
		frontier_epoch: 1,
		stage_epoch: 2,
		work_limit: 256,
		seconds_per_tick: ScalarValue(0.5),
	};
	let mut request_bytes = request.encode().unwrap();
	request_bytes[28..32].copy_from_slice(&3_u32.to_le_bytes());
	assert_eq!(
		SimulationStageRequest::decode(&request_bytes),
		Err(ProtocolError::ReservedSimulationStageField(3))
	);

	let invalid_limit = SimulationStageRequest {
		work_limit: 0,
		..request
	};
	assert_eq!(
		invalid_limit.encode(),
		Err(ProtocolError::InvalidStageWorkLimit(0))
	);
}

#[test]
fn mixture_state_batch_round_trips_exact_fixed_records() {
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(12.5);
	gases[31] = ScalarValue(0.25);
	let entries = [MixtureStateMutation {
		handle: handle(7, 11),
		expected_revision: 4,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases,
	}];
	let mut bytes = Vec::new();
	encode_mixture_state_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + MIXTURE_STATE_MUTATION_LEN);
	assert_eq!(&bytes[4..12], &handle(7, 11).encode());
	assert_eq!(&bytes[12..16], &4_u32.to_le_bytes());
	assert_eq!(&bytes[16..20], &[0; 4]);
	assert_eq!(decode_mixture_state_batch(&bytes, 8).unwrap(), entries);

	bytes.push(0);
	assert!(matches!(
		decode_mixture_state_batch(&bytes, 8),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn mixture_state_upload_frames_round_trip_with_offsets() {
	let begin = MixtureStateUploadBeginRequest {
		expected_count: 228,
	};
	assert_eq!(
		MixtureStateUploadBeginRequest::decode(&begin.encode().unwrap()),
		Ok(begin)
	);

	let entry = MixtureStateMutation {
		handle: handle(7, 11),
		expected_revision: 4,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
	};
	let append = MixtureStateUploadAppendRequest {
		upload_id: 41,
		offset: 227,
		mutations: vec![entry],
	};
	let append_bytes = append.encode().unwrap();
	assert_eq!(
		MixtureStateUploadAppendRequest::decode(&append_bytes, 4096).unwrap(),
		append
	);

	let commit = MixtureStateUploadCommitRequest { upload_id: 41 };
	assert_eq!(
		MixtureStateUploadCommitRequest::decode(&commit.encode()),
		Ok(commit)
	);
}

#[test]
fn mixture_state_upload_frames_reject_ambiguous_or_empty_identifiers() {
	assert_eq!(
		MixtureStateUploadBeginRequest { expected_count: 0 }.encode(),
		Err(ProtocolError::InvalidMixtureStateUploadCount(0))
	);
	let mut reserved_begin = MixtureStateUploadBeginRequest { expected_count: 1 }
		.encode()
		.unwrap();
	reserved_begin[4..8].copy_from_slice(&1_u32.to_le_bytes());
	assert_eq!(
		MixtureStateUploadBeginRequest::decode(&reserved_begin),
		Err(ProtocolError::ReservedMixtureStateUploadField(1))
	);
	assert!(matches!(
		MixtureStateUploadAppendRequest {
			upload_id: 1,
			offset: 0,
			mutations: Vec::new(),
		}
		.encode(),
		Err(ProtocolError::InvalidMixtureStateUploadCount(0))
	));
	assert_eq!(
		MixtureStateUploadCommitRequest::decode(&0_u64.to_le_bytes()),
		Err(ProtocolError::InvalidMixtureStateUploadId)
	);
	assert_eq!(
		MixtureStateUploadAbortRequest::decode(&0_u64.to_le_bytes()),
		Err(ProtocolError::InvalidMixtureStateUploadId)
	);
}

#[test]
fn mixture_state_batch_matches_complete_protocol_golden_bytes() {
	const GOLDEN_HEX: &str = concat!(
		"01000000070000000b0000000400000000000000000000000000104000000000",
		"00002040000000000000f03f0000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"0000000000000000000000000000000000000000000000000000000000000000",
		"00000040",
	);
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(1.0);
	gases[31] = ScalarValue(2.0);
	let entry = MixtureStateMutation {
		handle: handle(7, 11),
		expected_revision: 4,
		temperature: ScalarValue(4.0),
		volume: ScalarValue(8.0),
		gases,
	};
	let mut encoded = Vec::new();
	encode_mixture_state_batch(&[entry], &mut encoded).unwrap();
	let golden = decode_hex_fixture(GOLDEN_HEX);
	assert_eq!(golden.len(), 4 + MIXTURE_STATE_MUTATION_LEN);
	assert_eq!(encoded, golden);
	assert_eq!(decode_mixture_state_batch(&golden, 1).unwrap(), [entry]);
}

#[test]
fn mixture_state_batch_rejects_reserved_and_non_finite_values() {
	let entry = MixtureStateMutation {
		handle: handle(1, 2),
		expected_revision: 0,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		gases: [ScalarValue(0.0); MAX_GAS_SLOTS],
	};
	let mut bytes = Vec::new();
	encode_mixture_state_batch(&[entry], &mut bytes).unwrap();
	bytes[16..20].copy_from_slice(&1_u32.to_le_bytes());
	assert_eq!(
		decode_mixture_state_batch(&bytes, 1),
		Err(ProtocolError::ReservedMixtureStateField(1))
	);

	bytes[16..20].fill(0);
	bytes[20..28].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
	assert_eq!(
		decode_mixture_state_batch(&bytes, 1),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn mixture_snapshot_request_requires_one_exact_handle() {
	let request = MixtureSnapshotRequest {
		handle: handle(7, 11),
	};
	let bytes = request.encode();
	assert_eq!(MixtureSnapshotRequest::decode(&bytes), Ok(request));
	assert!(matches!(
		MixtureSnapshotRequest::decode(&bytes[..7]),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
	let mut trailing = bytes.to_vec();
	trailing.push(0);
	assert!(matches!(
		MixtureSnapshotRequest::decode(&trailing),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn mixture_snapshot_has_a_fixed_cross_bitness_layout() {
	let mut gases = [ScalarValue(0.0); MAX_GAS_SLOTS];
	gases[0] = ScalarValue(4.5);
	gases[1] = ScalarValue(9.25);
	let snapshot = MixtureSnapshot {
		revision: 0x1122_3344,
		gas_count: 2,
		temperature: ScalarValue(293.15),
		volume: ScalarValue(2500.0),
		minimum_heat_capacity: ScalarValue(80.0),
		total_moles: ScalarValue(13.75),
		pressure: ScalarValue(1.5),
		heat_capacity: ScalarValue(275.0),
		immutable: true,
		gases,
	};
	let bytes = snapshot.encode().unwrap();
	assert_eq!(bytes.len(), MIXTURE_SNAPSHOT_LEN);
	assert_eq!(&bytes[0..4], &0x1122_3344_u32.to_le_bytes());
	assert_eq!(&bytes[4..8], &2_u32.to_le_bytes());
	assert_eq!(&bytes[24..32], &80.0_f64.to_le_bytes());
	assert_eq!(&bytes[32..36], &1_u32.to_le_bytes());
	assert_eq!(&bytes[40..48], &13.75_f64.to_le_bytes());
	assert_eq!(&bytes[48..56], &1.5_f64.to_le_bytes());
	assert_eq!(&bytes[56..64], &275.0_f64.to_le_bytes());
	assert_eq!(MixtureSnapshot::decode(&bytes), Ok(snapshot));
}

#[test]
fn mixture_snapshot_rejects_invalid_gas_counts_and_non_finite_values() {
	let mut bytes = [0_u8; MIXTURE_SNAPSHOT_LEN];
	bytes[4..8].copy_from_slice(&((MAX_GAS_SLOTS as u32) + 1).to_le_bytes());
	assert!(matches!(
		MixtureSnapshot::decode(&bytes),
		Err(ProtocolError::GasCountExceeded { .. })
	));
	bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());
	bytes[8..16].copy_from_slice(&f64::NAN.to_bits().to_le_bytes());
	assert_eq!(
		MixtureSnapshot::decode(&bytes),
		Err(ProtocolError::NonFiniteScalar)
	);
	bytes[8..16].copy_from_slice(&293.15_f64.to_le_bytes());
	bytes[32..36].copy_from_slice(&2_u32.to_le_bytes());
	assert_eq!(
		MixtureSnapshot::decode(&bytes),
		Err(ProtocolError::UnknownMixtureSnapshotFlags(2))
	);
}

#[test]
fn lifecycle_batch_round_trips_and_checks_exact_counted_length() {
	let entries = [
		LifecycleMutation {
			action: LifecycleAction::Register,
			handle: handle(1, 2),
		},
		LifecycleMutation {
			action: LifecycleAction::Unregister,
			handle: handle(3, 4),
		},
	];
	let mut bytes = Vec::new();
	dogmos_protocol::encode_lifecycle_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + entries.len() * LIFECYCLE_MUTATION_LEN);
	assert_eq!(decode_lifecycle_batch(&bytes, 8).unwrap(), entries);
	bytes.push(0);
	assert!(matches!(
		decode_lifecycle_batch(&bytes, 8),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn lifecycle_batch_rejects_unknown_actions_and_capacity_overruns() {
	let mut bytes = vec![0_u8; 4 + LIFECYCLE_MUTATION_LEN];
	bytes[0..4].copy_from_slice(&1_u32.to_le_bytes());
	bytes[4..8].copy_from_slice(&99_u32.to_le_bytes());
	assert_eq!(
		decode_lifecycle_batch(&bytes, 1),
		Err(ProtocolError::UnknownLifecycleAction(99))
	);
	bytes[0..4].copy_from_slice(&2_u32.to_le_bytes());
	assert!(matches!(
		decode_lifecycle_batch(&bytes, 1),
		Err(ProtocolError::OperationCountExceeded { .. })
	));
	bytes.truncate(4);
	bytes[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
	assert!(matches!(
		decode_lifecycle_batch(&bytes, u32::MAX),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn adjacency_batch_round_trips_and_rejects_non_finite_conductivity() {
	let entries = [AdjacencyMutation {
		left: handle(1, 2),
		right: handle(3, 4),
		conductivity: ScalarValue(0.75),
	}];
	let mut bytes = Vec::new();
	dogmos_protocol::encode_adjacency_batch(&entries, &mut bytes).unwrap();
	assert_eq!(bytes.len(), 4 + ADJACENCY_MUTATION_LEN);
	assert_eq!(decode_adjacency_batch(&bytes, 4).unwrap(), entries);
	bytes[20..28].copy_from_slice(&f64::INFINITY.to_bits().to_le_bytes());
	assert_eq!(
		decode_adjacency_batch(&bytes, 4),
		Err(ProtocolError::NonFiniteScalar)
	);
}

#[test]
fn simulation_stage_request_is_fixed_width_and_validated() {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessTurfHeat,
		frontier_epoch: 0x0102_0304_0506_0708,
		stage_epoch: 0x1112_1314_1516_1718,
		work_limit: 256,
		seconds_per_tick: ScalarValue(0.5),
	};
	let bytes = request.encode().unwrap();
	assert_eq!(bytes.len(), SIMULATION_STAGE_REQUEST_LEN);
	assert_eq!(SimulationStageRequest::decode(&bytes), Ok(request));
	assert_eq!(&bytes[4..8], &[0; 4]);
	assert_eq!(&bytes[8..16], &request.frontier_epoch.to_le_bytes());
	assert_eq!(&bytes[16..24], &request.stage_epoch.to_le_bytes());
	assert_eq!(&bytes[24..28], &256_u32.to_le_bytes());
	assert_eq!(&bytes[28..32], &[0; 4]);
	assert_eq!(&bytes[32..40], &0.5_f64.to_le_bytes());
	let mut unknown = bytes;
	unknown[0..4].copy_from_slice(&99_u32.to_le_bytes());
	assert_eq!(
		SimulationStageRequest::decode(&unknown),
		Err(ProtocolError::UnknownSimulationStage(99))
	);
}

#[test]
fn reaction_stage_has_a_stable_wire_discriminant() {
	let request = SimulationStageRequest {
		stage: SimulationStage::ProcessReactions,
		frontier_epoch: 1,
		stage_epoch: 2,
		work_limit: 1,
		seconds_per_tick: ScalarValue(0.5),
	};
	let bytes = request.encode().unwrap();
	assert_eq!(&bytes[0..4], &5_u32.to_le_bytes());
	assert_eq!(SimulationStageRequest::decode(&bytes), Ok(request));
}

#[test]
fn simulation_stage_response_is_fixed_width() {
	let response = SimulationStageResponse {
		work_items: 64,
		callback_events: 3,
		pending: true,
		remaining_estimate: 129,
		produced_equalize_seeds: 7,
		produced_group_seeds: 5,
		produced_heat_seeds: 2,
	};
	let bytes = response.encode();
	assert_eq!(bytes.len(), SIMULATION_STAGE_RESPONSE_LEN);
	assert_eq!(SimulationStageResponse::decode(&bytes), Ok(response));
	assert!(matches!(
		SimulationStageResponse::decode(&bytes[..31]),
		Err(ProtocolError::InvalidPayloadLength { .. })
	));
}

#[test]
fn reaction_progress_uses_the_final_eight_bytes_for_its_transaction() {
	let response = MixtureCommandResponse::ReactionProgress {
		flags: 1,
		work_items: 17,
		pending: true,
		transaction_id: 0x0102_0304_0506_0708,
	};
	let bytes = response.encode().unwrap();
	assert_eq!(&bytes[8..12], &17_u32.to_le_bytes());
	assert_eq!(&bytes[12..16], &1_u32.to_le_bytes());
	assert_eq!(&bytes[16..24], &0x0102_0304_0506_0708_u64.to_le_bytes());
	assert_eq!(MixtureCommandResponse::decode(&bytes), Ok(response));

	let opaque_bits = MixtureCommandResponse::ReactionProgress {
		flags: 1,
		work_items: 17,
		pending: true,
		transaction_id: f64::NAN.to_bits(),
	};
	let opaque_bytes = opaque_bits.encode().unwrap();
	assert_eq!(
		MixtureCommandResponse::decode(&opaque_bytes),
		Ok(opaque_bits)
	);
}
