use crate::ProtocolError;

pub const CALLBACK_EVENT_KIND_COUNT: usize = 7;
pub const SERVICE_PROCESS_RSS_AVAILABLE: u32 = 1 << 0;
pub const SERVICE_PROCESS_CPU_AVAILABLE: u32 = 1 << 1;
pub const SERVICE_PROCESS_ALL_AVAILABLE: u32 =
	SERVICE_PROCESS_RSS_AVAILABLE | SERVICE_PROCESS_CPU_AVAILABLE;
pub const SERVICE_TELEMETRY_LEN: usize = 272;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceTelemetry {
	pub callback_depth: u32,
	pub callback_capacity: u32,
	pub callback_high_water: u32,
	pub continuation_depth: u32,
	pub continuation_capacity: u32,
	pub continuation_high_water: u32,
	pub oldest_callback_age_ticks: u64,
	pub callback_enqueued: u64,
	pub callback_drained: u64,
	pub callback_rejected: u64,
	pub continuation_timeouts: u64,
	pub request_timeouts: u64,
	pub protocol_errors: u64,
	pub callback_enqueued_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	pub callback_drained_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	pub callback_rejected_by_kind: [u64; CALLBACK_EVENT_KIND_COUNT],
	pub service_process_available_flags: u32,
	pub service_rss_bytes: u64,
	pub service_cpu_total_milliseconds: u64,
}

impl ServiceTelemetry {
	pub fn encode(self) -> [u8; SERVICE_TELEMETRY_LEN] {
		let mut output = [0_u8; SERVICE_TELEMETRY_LEN];
		output[0..4].copy_from_slice(&self.callback_depth.to_le_bytes());
		output[4..8].copy_from_slice(&self.callback_capacity.to_le_bytes());
		output[8..12].copy_from_slice(&self.callback_high_water.to_le_bytes());
		output[12..16].copy_from_slice(&self.continuation_depth.to_le_bytes());
		output[16..20].copy_from_slice(&self.continuation_capacity.to_le_bytes());
		output[20..24].copy_from_slice(&self.continuation_high_water.to_le_bytes());
		output[24..32].copy_from_slice(&self.oldest_callback_age_ticks.to_le_bytes());
		output[32..40].copy_from_slice(&self.callback_enqueued.to_le_bytes());
		output[40..48].copy_from_slice(&self.callback_drained.to_le_bytes());
		output[48..56].copy_from_slice(&self.callback_rejected.to_le_bytes());
		output[56..64].copy_from_slice(&self.continuation_timeouts.to_le_bytes());
		output[64..72].copy_from_slice(&self.request_timeouts.to_le_bytes());
		output[72..80].copy_from_slice(&self.protocol_errors.to_le_bytes());
		encode_counters(&mut output[80..136], self.callback_enqueued_by_kind);
		encode_counters(&mut output[136..192], self.callback_drained_by_kind);
		encode_counters(&mut output[192..248], self.callback_rejected_by_kind);
		output[248..252].copy_from_slice(&self.service_process_available_flags.to_le_bytes());
		output[256..264].copy_from_slice(&self.service_rss_bytes.to_le_bytes());
		output[264..272].copy_from_slice(&self.service_cpu_total_milliseconds.to_le_bytes());
		output
	}

	pub fn decode(input: &[u8]) -> Result<Self, ProtocolError> {
		if input.len() != SERVICE_TELEMETRY_LEN {
			return Err(ProtocolError::InvalidPayloadLength {
				expected: SERVICE_TELEMETRY_LEN as u32,
				actual: input.len() as u32,
			});
		}
		let service_process_available_flags = read_u32(input, 248);
		if service_process_available_flags & !SERVICE_PROCESS_ALL_AVAILABLE != 0 {
			return Err(ProtocolError::UnknownServiceProcessFlags(
				service_process_available_flags,
			));
		}
		let reserved = read_u32(input, 252);
		if reserved != 0 {
			return Err(ProtocolError::ReservedServiceTelemetryField(reserved));
		}
		let service_rss_bytes = read_u64(input, 256);
		let service_cpu_total_milliseconds = read_u64(input, 264);
		if service_process_available_flags & SERVICE_PROCESS_RSS_AVAILABLE == 0
			&& service_rss_bytes != 0
			|| service_process_available_flags & SERVICE_PROCESS_CPU_AVAILABLE == 0
				&& service_cpu_total_milliseconds != 0
		{
			return Err(ProtocolError::NonZeroUnavailableServiceProcessMetric);
		}
		Ok(Self {
			callback_depth: read_u32(input, 0),
			callback_capacity: read_u32(input, 4),
			callback_high_water: read_u32(input, 8),
			continuation_depth: read_u32(input, 12),
			continuation_capacity: read_u32(input, 16),
			continuation_high_water: read_u32(input, 20),
			oldest_callback_age_ticks: read_u64(input, 24),
			callback_enqueued: read_u64(input, 32),
			callback_drained: read_u64(input, 40),
			callback_rejected: read_u64(input, 48),
			continuation_timeouts: read_u64(input, 56),
			request_timeouts: read_u64(input, 64),
			protocol_errors: read_u64(input, 72),
			callback_enqueued_by_kind: decode_counters(&input[80..136]),
			callback_drained_by_kind: decode_counters(&input[136..192]),
			callback_rejected_by_kind: decode_counters(&input[192..248]),
			service_process_available_flags,
			service_rss_bytes,
			service_cpu_total_milliseconds,
		})
	}
}

fn encode_counters(output: &mut [u8], counters: [u64; CALLBACK_EVENT_KIND_COUNT]) {
	for (index, counter) in counters.into_iter().enumerate() {
		let offset = index * 8;
		output[offset..offset + 8].copy_from_slice(&counter.to_le_bytes());
	}
}

fn decode_counters(input: &[u8]) -> [u64; CALLBACK_EVENT_KIND_COUNT] {
	let mut counters = [0_u64; CALLBACK_EVENT_KIND_COUNT];
	for (index, counter) in counters.iter_mut().enumerate() {
		*counter = read_u64(input, index * 8);
	}
	counters
}

fn read_u32(input: &[u8], offset: usize) -> u32 {
	u32::from_le_bytes(
		input[offset..offset + 4]
			.try_into()
			.expect("validated length"),
	)
}

fn read_u64(input: &[u8], offset: usize) -> u64 {
	u64::from_le_bytes(
		input[offset..offset + 8]
			.try_into()
			.expect("validated length"),
	)
}
