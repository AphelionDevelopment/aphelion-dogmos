use byondapi::prelude::*;
use coarsetime::{Duration, Instant};
use eyre::Result;
use std::convert::TryInto;
use std::sync::{
	atomic::{AtomicBool, AtomicUsize, Ordering},
	RwLock,
};

type DeferredFunc = Box<dyn FnOnce() -> Result<()> + Send + Sync>;

struct DeferredCallback {
	callback: DeferredFunc,
	owned_bytes_lower_bound: usize,
}

type CallbackChannel = (
	flume::Sender<DeferredCallback>,
	flume::Receiver<DeferredCallback>,
);

static CALLBACK_CHANNEL: std::sync::OnceLock<CallbackChannel> = std::sync::OnceLock::new();
static CALLBACK_ENQUEUE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ITEMS_ENQUEUED: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ITEMS_DRAINED: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_QUEUE_DEPTH_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_OWNED_BYTES_CURRENT: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_OWNED_BYTES_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_OWNED_BYTES_ENQUEUED: AtomicUsize = AtomicUsize::new(0);
/// Maximum queued callbacks in the legacy 32-bit backend.
pub const MAX_CALLBACK_ITEMS: usize = 65_536;
/// Maximum producer-accounted callback storage; excludes allocator overhead.
pub const MAX_CALLBACK_ACCOUNTED_BYTES: usize = 32 * 1024 * 1024;
static CALLBACK_FAILED: AtomicBool = AtomicBool::new(false);
static CALLBACK_STATE: RwLock<bool> = RwLock::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueCallbackError {
	ShuttingDown,
	Disconnected,
	CapacityExceeded,
}

impl std::fmt::Display for QueueCallbackError {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(formatter, "{self:?}")
	}
}

impl std::error::Error for QueueCallbackError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackMetrics {
	pub items_enqueued: usize,
	pub items_drained: usize,
	pub queue_depth: usize,
	pub queue_depth_high_water: usize,
	pub owned_bytes_lower_bound_current: usize,
	pub owned_bytes_lower_bound_high_water: usize,
	pub owned_bytes_lower_bound_enqueued: usize,
	pub enqueue_failures: usize,
}

/// Returns whether teardown has closed admission for the previous world.
pub fn callbacks_closed() -> bool {
	!*CALLBACK_STATE.read().expect("callback state lock poisoned")
}

/// Reopens the main-thread callback queue for a new BYOND world.
pub fn begin_callbacks() {
	let mut state = CALLBACK_STATE
		.write()
		.expect("callback state lock poisoned");
	if let Some((_, receiver)) = CALLBACK_CHANNEL.get() {
		receiver.drain().for_each(std::mem::drop);
	}
	CALLBACK_FAILED.store(false, Ordering::Release);
	CALLBACK_ENQUEUE_FAILURES.store(0, Ordering::Relaxed);
	CALLBACK_ITEMS_ENQUEUED.store(0, Ordering::Relaxed);
	CALLBACK_ITEMS_DRAINED.store(0, Ordering::Relaxed);
	CALLBACK_QUEUE_DEPTH_HIGH_WATER.store(0, Ordering::Relaxed);
	CALLBACK_OWNED_BYTES_CURRENT.store(0, Ordering::Relaxed);
	CALLBACK_OWNED_BYTES_HIGH_WATER.store(0, Ordering::Relaxed);
	CALLBACK_OWNED_BYTES_ENQUEUED.store(0, Ordering::Relaxed);
	*state = true;
}

/// Rejects new callbacks and drains callbacks that were queued before teardown began.
pub fn clean_callbacks() {
	let mut state = CALLBACK_STATE
		.write()
		.expect("callback state lock poisoned");
	*state = false;
	if let Some((_, rx)) = CALLBACK_CHANNEL.get() {
		for callback in rx.drain() {
			release_owned_bytes(callback.owned_bytes_lower_bound);
		}
	};
}

fn with_callback_receiver<T>(f: impl Fn(&flume::Receiver<DeferredCallback>) -> T) -> T {
	f(&CALLBACK_CHANNEL
		.get_or_init(|| flume::bounded(MAX_CALLBACK_ITEMS))
		.1)
}

fn saturating_add(counter: &AtomicUsize, value: usize) -> usize {
	counter
		.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
			Some(current.saturating_add(value))
		})
		.unwrap_or_else(|current| current)
		.saturating_add(value)
}

fn release_owned_bytes(bytes: usize) {
	let _ = CALLBACK_OWNED_BYTES_CURRENT.fetch_update(
		Ordering::Relaxed,
		Ordering::Relaxed,
		|current| Some(current.saturating_sub(bytes)),
	);
}

/// Queues a main-thread callback without silently discarding it when the channel is healthy.
///
/// Overflow latches a fatal error until the next world. Producers never block the main thread,
/// and the FFI boundary rejects further simulation work after a critical callback is rejected.
pub fn queue_callback(
	callback: DeferredFunc,
	owned_bytes: usize,
) -> std::result::Result<(), QueueCallbackError> {
	let state = CALLBACK_STATE
		.write()
		.expect("callback state lock poisoned");
	if !*state {
		saturating_add(&CALLBACK_ENQUEUE_FAILURES, 1);
		return Err(QueueCallbackError::ShuttingDown);
	}
	let owned_bytes_lower_bound = owned_bytes.saturating_add(std::mem::size_of::<DeferredFunc>());
	if CALLBACK_FAILED.load(Ordering::Acquire)
		|| CALLBACK_CHANNEL
			.get()
			.is_some_and(|channel| channel.0.len() >= MAX_CALLBACK_ITEMS)
		|| owned_bytes_lower_bound
			> MAX_CALLBACK_ACCOUNTED_BYTES
				.saturating_sub(CALLBACK_OWNED_BYTES_CURRENT.load(Ordering::Relaxed))
	{
		CALLBACK_FAILED.store(true, Ordering::Release);
		saturating_add(&CALLBACK_ENQUEUE_FAILURES, 1);
		return Err(QueueCallbackError::CapacityExceeded);
	}
	let current_owned_bytes =
		saturating_add(&CALLBACK_OWNED_BYTES_CURRENT, owned_bytes_lower_bound);
	let envelope = DeferredCallback {
		callback,
		owned_bytes_lower_bound,
	};
	if CALLBACK_CHANNEL
		.get_or_init(|| flume::bounded(MAX_CALLBACK_ITEMS))
		.0
		.try_send(envelope)
		.is_err()
	{
		release_owned_bytes(owned_bytes_lower_bound);
		saturating_add(&CALLBACK_ENQUEUE_FAILURES, 1);
		CALLBACK_FAILED.store(true, Ordering::Release);
		Err(QueueCallbackError::Disconnected)
	} else {
		saturating_add(&CALLBACK_ITEMS_ENQUEUED, 1);
		saturating_add(&CALLBACK_OWNED_BYTES_ENQUEUED, owned_bytes_lower_bound);
		CALLBACK_OWNED_BYTES_HIGH_WATER.fetch_max(current_owned_bytes, Ordering::Relaxed);
		let depth = CALLBACK_CHANNEL.get().map_or(0, |channel| channel.0.len());
		CALLBACK_QUEUE_DEPTH_HIGH_WATER.fetch_max(depth, Ordering::Relaxed);
		Ok(())
	}
}

/// Rejects simulation after callback capacity or transport failure until world reinitialization.
pub fn ensure_callbacks_healthy() -> Result<()> {
	if CALLBACK_FAILED.load(Ordering::Acquire) {
		return Err(eyre::eyre!("Dogmos legacy callback queue failed: capacity exceeded or disconnected; atmosphere processing is stopped until world restart"));
	}
	Ok(())
}

/// Returns the number of callbacks rejected because the main-thread queue was already closed.
pub fn callback_enqueue_failures() -> usize {
	CALLBACK_ENQUEUE_FAILURES.load(Ordering::Relaxed)
}

#[must_use]
pub fn callback_metrics() -> CallbackMetrics {
	let queue_depth = CALLBACK_CHANNEL.get().map_or(0, |channel| channel.1.len());
	CallbackMetrics {
		items_enqueued: CALLBACK_ITEMS_ENQUEUED.load(Ordering::Relaxed),
		items_drained: CALLBACK_ITEMS_DRAINED.load(Ordering::Relaxed),
		queue_depth,
		queue_depth_high_water: CALLBACK_QUEUE_DEPTH_HIGH_WATER.load(Ordering::Relaxed),
		owned_bytes_lower_bound_current: CALLBACK_OWNED_BYTES_CURRENT.load(Ordering::Relaxed),
		owned_bytes_lower_bound_high_water: CALLBACK_OWNED_BYTES_HIGH_WATER.load(Ordering::Relaxed),
		owned_bytes_lower_bound_enqueued: CALLBACK_OWNED_BYTES_ENQUEUED.load(Ordering::Relaxed),
		enqueue_failures: CALLBACK_ENQUEUE_FAILURES.load(Ordering::Relaxed),
	}
}

fn report_callback_error(error: impl std::fmt::Debug) {
	let Ok(error_string) = format!("{error:?}").try_into() else {
		return;
	};
	let _ = byondapi::global_call::call_global_id(
		byond_string!("byondapi_stack_trace"),
		&[error_string],
	);
}

/// Runs every outstanding callback.
fn process_callbacks() {
	if ensure_callbacks_healthy().is_err() {
		return;
	}
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			release_owned_bytes(callback.owned_bytes_lower_bound);
			if CALLBACK_FAILED.load(Ordering::Acquire) {
				break;
			}
			saturating_add(&CALLBACK_ITEMS_DRAINED, 1);
			if let Err(error) = (callback.callback)() {
				report_callback_error(error);
			}
		}
	})
}

/// Runs callbacks until the time limit is reached.
fn process_callbacks_for(duration: Duration) -> bool {
	if ensure_callbacks_healthy().is_err() {
		return false;
	}
	let timer = Instant::now();
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			if CALLBACK_FAILED.load(Ordering::Acquire) {
				release_owned_bytes(callback.owned_bytes_lower_bound);
				return false;
			}
			release_owned_bytes(callback.owned_bytes_lower_bound);
			saturating_add(&CALLBACK_ITEMS_DRAINED, 1);
			if let Err(e) = (callback.callback)() {
				report_callback_error(e);
			}
			if timer.elapsed() >= duration {
				return true;
			}
		}
		false
	})
}

/// Goes through every single outstanding callback and calls them, until a given time limit in milliseconds is reached.
pub fn process_callbacks_for_millis(millis: u64) -> bool {
	process_callbacks_for(Duration::from_millis(millis))
}

/// This function is to be called from byond, preferably once a tick.
/// Calling with no arguments will process every outstanding callback.
/// Calling with one argument will process the callbacks until a given time limit is reached.
/// Time limit is in milliseconds.
/// This has to be manually hooked in the code, e.g.
/// ```ignore
/// #[bind("/proc/process_atmos_callbacks")]
/// fn atmos_callback_handle(remaining: ByondValue) {
///     auxcallback::callback_processing_hook(remaining)
/// }
/// ```
pub fn callback_processing_hook(time_remaining: ByondValue) -> Result<ByondValue> {
	ensure_callbacks_healthy()?;
	if time_remaining.is_num() {
		let limit = time_remaining.get_number()?.max(0.0) as u64;
		Ok(process_callbacks_for_millis(limit).into())
	} else {
		process_callbacks();
		Ok(ByondValue::null())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{
		atomic::{AtomicUsize, Ordering},
		Arc, Mutex,
	};

	static CALLBACK_TEST_LOCK: Mutex<()> = Mutex::new(());

	#[test]
	fn callback_capacity_is_exact_and_failure_prevents_queued_execution() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let executed = Arc::new(AtomicUsize::new(0));
		let capture = Arc::clone(&executed);
		queue_callback(
			Box::new(move || {
				capture.fetch_add(1, Ordering::SeqCst);
				Ok(())
			}),
			MAX_CALLBACK_ACCOUNTED_BYTES - std::mem::size_of::<DeferredFunc>(),
		)
		.unwrap();
		assert_eq!(
			queue_callback(Box::new(|| Ok(())), 1),
			Err(QueueCallbackError::CapacityExceeded)
		);
		process_callbacks();
		assert_eq!(executed.load(Ordering::SeqCst), 0);
		assert!(ensure_callbacks_healthy().is_err());
		clean_callbacks();
		begin_callbacks();
		assert!(ensure_callbacks_healthy().is_ok());
		for _ in 0..MAX_CALLBACK_ITEMS {
			queue_callback(Box::new(|| Ok(())), 0).unwrap();
		}
		assert_eq!(callback_metrics().queue_depth, MAX_CALLBACK_ITEMS);
		assert_eq!(
			queue_callback(Box::new(|| Ok(())), 0),
			Err(QueueCallbackError::CapacityExceeded)
		);
		assert_eq!(callback_metrics().queue_depth, MAX_CALLBACK_ITEMS);
		clean_callbacks();
		begin_callbacks();
	}

	#[test]
	fn queued_callbacks_are_delivered_in_order() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let observed = Arc::new(AtomicUsize::new(0));
		for expected in 1..=3 {
			let observed = Arc::clone(&observed);
			queue_callback(
				Box::new(move || {
					assert_eq!(observed.fetch_add(1, Ordering::SeqCst) + 1, expected);
					Ok(())
				}),
				0,
			)
			.unwrap();
		}

		assert!(!process_callbacks_for_millis(100));
		assert_eq!(observed.load(Ordering::SeqCst), 3);
	}

	#[test]
	fn callbacks_are_rejected_after_cleanup() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let failures_before_cleanup = callback_enqueue_failures();
		clean_callbacks();
		assert_eq!(
			queue_callback(Box::new(|| Ok(())), 0),
			Err(QueueCallbackError::ShuttingDown)
		);
		assert_eq!(callback_enqueue_failures(), failures_before_cleanup + 1);
		begin_callbacks();
	}

	#[test]
	fn callback_metrics_track_queue_depth_and_owned_handle_bytes() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let before = callback_metrics();
		queue_callback(Box::new(|| Ok(())), 0).unwrap();
		let queued = callback_metrics();
		assert_eq!(queued.queue_depth, before.queue_depth + 1);
		assert!(queued.owned_bytes_lower_bound_current > before.owned_bytes_lower_bound_current);
		assert!(queued.queue_depth_high_water >= queued.queue_depth);
		assert!(!process_callbacks_for_millis(100));
		let drained = callback_metrics();
		assert_eq!(drained.queue_depth, before.queue_depth);
		assert_eq!(
			drained.owned_bytes_lower_bound_current,
			before.owned_bytes_lower_bound_current
		);
		assert!(drained.items_drained > before.items_drained);
	}

	#[test]
	fn callback_metrics_include_transferred_vector_capacity() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let payload = Vec::<u64>::with_capacity(128);
		let payload_bytes = payload.capacity() * std::mem::size_of::<u64>();
		let before = callback_metrics();
		queue_callback(
			Box::new(move || {
				drop(payload);
				Ok(())
			}),
			payload_bytes,
		)
		.unwrap();

		let queued = callback_metrics();
		assert!(
			queued.owned_bytes_lower_bound_current
				>= before
					.owned_bytes_lower_bound_current
					.saturating_add(payload_bytes)
					.saturating_add(std::mem::size_of::<DeferredFunc>())
		);
		assert!(!process_callbacks_for_millis(100));
		let drained = callback_metrics();
		assert_eq!(
			drained.owned_bytes_lower_bound_current,
			before.owned_bytes_lower_bound_current
		);
		assert!(drained.owned_bytes_lower_bound_enqueued >= queued.owned_bytes_lower_bound_current);
	}

	#[test]
	fn callback_metrics_saturate_and_world_cleanup_resets_ownership() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		CALLBACK_ITEMS_ENQUEUED.store(usize::MAX, Ordering::Relaxed);
		CALLBACK_ITEMS_DRAINED.store(usize::MAX, Ordering::Relaxed);
		assert_eq!(
			queue_callback(Box::new(|| Ok(())), usize::MAX),
			Err(QueueCallbackError::CapacityExceeded)
		);
		assert!(ensure_callbacks_healthy().is_err());
		assert_eq!(
			queue_callback(Box::new(|| Ok(())), 0),
			Err(QueueCallbackError::CapacityExceeded)
		);
		assert_eq!(callback_metrics().owned_bytes_lower_bound_current, 0);
		clean_callbacks();
		assert_eq!(callback_metrics().owned_bytes_lower_bound_current, 0);
		begin_callbacks();
		let reset = callback_metrics();
		assert_eq!(reset.items_enqueued, 0);
		assert_eq!(reset.items_drained, 0);
		assert_eq!(reset.owned_bytes_lower_bound_high_water, 0);
		assert_eq!(reset.owned_bytes_lower_bound_enqueued, 0);
	}
}
