#![cfg(windows)]

use dogmos_byond::{BoundedDogmosClient, ClientError, DogmosClient};
use dogmos_protocol::{
	read_frame_into, write_frame, BuildIdentity, CapacityLimits, HandshakePayload, OperationKind,
	DOGMOS_ABI_VERSION, DOGMOS_PROTOCOL_VERSION, HANDSHAKE_PAYLOAD_LEN, MAX_CONTROL_PAYLOAD,
};
use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions};
use std::{
	thread,
	time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn handshake() -> HandshakePayload {
	HandshakePayload {
		auth_token: [0x5a; 32],
		identity: BuildIdentity {
			abi_version: DOGMOS_ABI_VERSION,
			protocol_version: DOGMOS_PROTOCOL_VERSION,
			source_revision: [1; 20],
			feature_fingerprint: [2; 32],
			executable_digest: [3; 32],
		},
		capacities: CapacityLimits {
			max_control_payload: MAX_CONTROL_PAYLOAD,
			max_batch_operations: 4096,
			max_callback_events: 1024,
			max_pending_continuations: 1024,
			max_world_bytes: 8 * 1024 * 1024 * 1024,
		},
		process_id: std::process::id(),
		world_generation: 1,
		world_nonce: 0x1234_5678_90ab_cdef,
	}
}

#[test]
fn stalled_read_releases_the_caller_and_cancels_the_worker() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-bounded-io-{}-{unique}", std::process::id());
	let name = endpoint.clone().to_ns_name::<GenericNamespaced>().unwrap();
	let listener = ListenerOptions::new().name(name).create_sync().unwrap();
	let expected = handshake();
	thread::spawn(move || {
		let mut stream = listener.accept().unwrap();
		let mut payload = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload).unwrap();
		assert_eq!(request.operation_kind().unwrap(), OperationKind::Handshake);
		assert_eq!(
			HandshakePayload::decode(&payload[..payload_len]).unwrap(),
			expected
		);
		let response = HandshakePayload {
			process_id: std::process::id(),
			..expected
		};
		write_frame(&mut stream, request.response(), &response.encode()).unwrap();
		let mut request_payload = [0_u8; 16];
		let _ = read_frame_into(&mut stream, &mut request_payload).unwrap();
		thread::sleep(Duration::from_secs(2));
	});

	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)).unwrap();
	let mut bounded = BoundedDogmosClient::new(client).unwrap();
	let started = Instant::now();
	let result = bounded.echo(b"stall", Duration::from_millis(25));
	assert!(matches!(result, Err(ClientError::RequestTimeout)));
	assert!(started.elapsed() < Duration::from_millis(250));
	assert!(matches!(
		bounded.echo(b"late", Duration::from_millis(25)),
		Err(ClientError::WorkerStopped)
	));

	let cancellation_deadline = Instant::now() + Duration::from_millis(500);
	while !bounded.is_worker_finished() && Instant::now() < cancellation_deadline {
		thread::sleep(Duration::from_millis(5));
	}
	assert!(bounded.is_worker_finished());
}

#[test]
fn bounded_worker_preserves_a_successful_round_trip() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-bounded-ok-{}-{unique}", std::process::id());
	let name = endpoint.clone().to_ns_name::<GenericNamespaced>().unwrap();
	let listener = ListenerOptions::new().name(name).create_sync().unwrap();
	let expected = handshake();
	thread::spawn(move || {
		let mut stream = listener.accept().unwrap();
		let mut payload = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload).unwrap();
		assert_eq!(
			HandshakePayload::decode(&payload[..payload_len]).unwrap(),
			expected
		);
		write_frame(&mut stream, request.response(), &expected.encode()).unwrap();

		let mut request_payload = [0_u8; 16];
		let (request, payload_len) = read_frame_into(&mut stream, &mut request_payload).unwrap();
		write_frame(
			&mut stream,
			request.response(),
			&request_payload[..payload_len],
		)
		.unwrap();
	});

	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)).unwrap();
	let mut bounded = BoundedDogmosClient::new(client).unwrap();
	assert_eq!(
		bounded.echo(b"bounded", Duration::from_secs(1)).unwrap(),
		b"bounded"
	);
}

#[test]
fn bounded_worker_reuses_fixed_transport_buffers() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-bounded-reuse-{}-{unique}", std::process::id());
	let name = endpoint.clone().to_ns_name::<GenericNamespaced>().unwrap();
	let listener = ListenerOptions::new().name(name).create_sync().unwrap();
	let expected = handshake();
	thread::spawn(move || {
		let mut stream = listener.accept().unwrap();
		let mut payload = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload).unwrap();
		assert_eq!(
			HandshakePayload::decode(&payload[..payload_len]).unwrap(),
			expected
		);
		write_frame(&mut stream, request.response(), &expected.encode()).unwrap();

		let mut request_payload = [0_u8; 16];
		for _ in 0..2 {
			let (request, payload_len) =
				read_frame_into(&mut stream, &mut request_payload).unwrap();
			write_frame(
				&mut stream,
				request.response(),
				&request_payload[..payload_len],
			)
			.unwrap();
		}
	});

	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)).unwrap();
	let mut bounded = BoundedDogmosClient::new(client).unwrap();
	assert_eq!(
		bounded.retained_buffer_capacities(),
		(MAX_CONTROL_PAYLOAD as usize, MAX_CONTROL_PAYLOAD as usize)
	);
	let first_pointer = bounded
		.round_trip(
			OperationKind::Echo,
			b"first",
			MAX_CONTROL_PAYLOAD as usize,
			Duration::from_secs(1),
		)
		.unwrap()
		.as_ptr();
	let second_pointer = bounded
		.round_trip(
			OperationKind::Echo,
			b"second",
			MAX_CONTROL_PAYLOAD as usize,
			Duration::from_secs(1),
		)
		.unwrap()
		.as_ptr();
	assert_eq!(first_pointer, second_pointer);
}

#[test]
fn corrupt_response_fails_closed_and_stops_the_worker() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-bounded-corrupt-{}-{unique}", std::process::id());
	let name = endpoint.clone().to_ns_name::<GenericNamespaced>().unwrap();
	let listener = ListenerOptions::new().name(name).create_sync().unwrap();
	let expected = handshake();
	thread::spawn(move || {
		let mut stream = listener.accept().unwrap();
		let mut payload = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload).unwrap();
		assert_eq!(
			HandshakePayload::decode(&payload[..payload_len]).unwrap(),
			expected
		);
		write_frame(&mut stream, request.response(), &expected.encode()).unwrap();

		let mut request_payload = [0_u8; 16];
		let (request, payload_len) = read_frame_into(&mut stream, &mut request_payload).unwrap();
		let mut corrupt = request.response();
		corrupt.request_id += 1;
		write_frame(&mut stream, corrupt, &request_payload[..payload_len]).unwrap();

		if let Ok((request, payload_len)) = read_frame_into(&mut stream, &mut request_payload) {
			write_frame(
				&mut stream,
				request.response(),
				&request_payload[..payload_len],
			)
			.unwrap();
		}
	});

	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)).unwrap();
	let mut bounded = BoundedDogmosClient::new(client).unwrap();
	assert!(matches!(
		bounded.echo(b"corrupt", Duration::from_secs(1)),
		Err(ClientError::Protocol(_))
	));
	assert!(matches!(
		bounded.echo(b"must-not-run", Duration::from_secs(1)),
		Err(ClientError::WorkerStopped)
	));
}

#[test]
fn service_disconnect_fails_closed_and_stops_the_worker() {
	let unique = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_nanos();
	let endpoint = format!("dogmos-bounded-death-{}-{unique}", std::process::id());
	let name = endpoint.clone().to_ns_name::<GenericNamespaced>().unwrap();
	let listener = ListenerOptions::new().name(name).create_sync().unwrap();
	let expected = handshake();
	thread::spawn(move || {
		let mut stream = listener.accept().unwrap();
		let mut payload = [0_u8; HANDSHAKE_PAYLOAD_LEN];
		let (request, payload_len) = read_frame_into(&mut stream, &mut payload).unwrap();
		assert_eq!(
			HandshakePayload::decode(&payload[..payload_len]).unwrap(),
			expected
		);
		write_frame(&mut stream, request.response(), &expected.encode()).unwrap();
		let mut request_payload = [0_u8; 16];
		let _ = read_frame_into(&mut stream, &mut request_payload).unwrap();
	});

	let client = DogmosClient::connect(&endpoint, expected, Duration::from_secs(1)).unwrap();
	let mut bounded = BoundedDogmosClient::new(client).unwrap();
	assert!(matches!(
		bounded.echo(b"disconnect", Duration::from_secs(1)),
		Err(ClientError::Io(_) | ClientError::Transport(_))
	));
	assert!(matches!(
		bounded.echo(b"must-not-run", Duration::from_secs(1)),
		Err(ClientError::WorkerStopped)
	));
}
