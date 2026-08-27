use byondapi::prelude::*;
use coarsetime::{Duration, Instant};
use eyre::Result;
use std::convert::TryInto;
use std::sync::{
	atomic::{AtomicUsize, Ordering},
	RwLock,
};

type DeferredFunc = Box<dyn FnOnce() -> Result<()> + Send + Sync>;
type CallbackChannel = (flume::Sender<DeferredFunc>, flume::Receiver<DeferredFunc>);
pub type CallbackSender = flume::Sender<DeferredFunc>;
pub type CallbackReceiver = flume::Receiver<DeferredFunc>;

static CALLBACK_CHANNEL: std::sync::OnceLock<CallbackChannel> = std::sync::OnceLock::new();
static CALLBACK_ENQUEUE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ITEMS_ENQUEUED: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_ITEMS_DRAINED: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_QUEUE_DEPTH_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);
static CALLBACK_STATE: RwLock<bool> = RwLock::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackMetrics {
	pub items_enqueued: usize,
	pub items_drained: usize,
	pub queue_depth: usize,
	pub queue_depth_high_water: usize,
	pub owned_bytes_current: usize,
	pub owned_bytes_enqueued: usize,
	pub enqueue_failures: usize,
}

/// Reopens the main-thread callback queue for a new BYOND world.
pub fn begin_callbacks() {
	let mut state = CALLBACK_STATE
		.write()
		.expect("callback state lock poisoned");
	CALLBACK_ENQUEUE_FAILURES.store(0, Ordering::Relaxed);
	*state = true;
}

/// Rejects new callbacks and drains callbacks that were queued before teardown began.
pub fn clean_callbacks() {
	let mut state = CALLBACK_STATE
		.write()
		.expect("callback state lock poisoned");
	*state = false;
	if let Some((_, rx)) = CALLBACK_CHANNEL.get() {
		rx.drain().for_each(std::mem::drop)
	};
}

fn with_callback_receiver<T>(f: impl Fn(&flume::Receiver<DeferredFunc>) -> T) -> T {
	f(&CALLBACK_CHANNEL.get_or_init(flume::unbounded).1)
}

/// Returns a copy of the callback sender for compatibility with existing integrations.
///
/// New callback producers must use [`queue_callback`] so teardown rejection is counted.
pub fn byond_callback_sender() -> flume::Sender<DeferredFunc> {
	CALLBACK_CHANNEL.get_or_init(flume::unbounded).0.clone()
}

/// Queues a main-thread callback without silently discarding it when the channel is healthy.
///
/// The callback channel is unbounded, so a live server cannot lose work to queue capacity. Once
/// teardown begins, new callbacks are rejected and counted instead of being silently discarded.
pub fn queue_callback(callback: DeferredFunc) {
	let state = CALLBACK_STATE.read().expect("callback state lock poisoned");
	if !*state {
		CALLBACK_ENQUEUE_FAILURES.fetch_add(1, Ordering::Relaxed);
		return;
	}
	if CALLBACK_CHANNEL
		.get_or_init(flume::unbounded)
		.0
		.send(callback)
		.is_err()
	{
		CALLBACK_ENQUEUE_FAILURES.fetch_add(1, Ordering::Relaxed);
	} else {
		CALLBACK_ITEMS_ENQUEUED.fetch_add(1, Ordering::Relaxed);
		let depth = CALLBACK_CHANNEL.get().map_or(0, |channel| channel.0.len());
		CALLBACK_QUEUE_DEPTH_HIGH_WATER.fetch_max(depth, Ordering::Relaxed);
	}
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
		owned_bytes_current: queue_depth.saturating_mul(std::mem::size_of::<DeferredFunc>()),
		owned_bytes_enqueued: CALLBACK_ITEMS_ENQUEUED
			.load(Ordering::Relaxed)
			.saturating_mul(std::mem::size_of::<DeferredFunc>()),
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
	with_callback_receiver(|receiver| {
		receiver
			.try_iter()
			.filter_map(|cb| {
				CALLBACK_ITEMS_DRAINED.fetch_add(1, Ordering::Relaxed);
				cb().err()
			})
			.for_each(report_callback_error)
	})
}

/// Runs callbacks until the time limit is reached.
fn process_callbacks_for(duration: Duration) -> bool {
	let timer = Instant::now();
	with_callback_receiver(|receiver| {
		for callback in receiver.try_iter() {
			CALLBACK_ITEMS_DRAINED.fetch_add(1, Ordering::Relaxed);
			if let Err(e) = callback() {
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
	fn queued_callbacks_are_delivered_in_order() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let observed = Arc::new(AtomicUsize::new(0));
		for expected in 1..=3 {
			let observed = Arc::clone(&observed);
			queue_callback(Box::new(move || {
				assert_eq!(observed.fetch_add(1, Ordering::SeqCst) + 1, expected);
				Ok(())
			}));
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
		queue_callback(Box::new(|| Ok(())));
		assert_eq!(callback_enqueue_failures(), failures_before_cleanup + 1);
		begin_callbacks();
	}

	#[test]
	fn callback_metrics_track_queue_depth_and_owned_handle_bytes() {
		let _guard = CALLBACK_TEST_LOCK.lock().unwrap();
		begin_callbacks();
		let before = callback_metrics();
		queue_callback(Box::new(|| Ok(())));
		let queued = callback_metrics();
		assert_eq!(queued.queue_depth, before.queue_depth + 1);
		assert!(queued.owned_bytes_current > before.owned_bytes_current);
		assert!(queued.queue_depth_high_water >= queued.queue_depth);
		assert!(!process_callbacks_for_millis(100));
		let drained = callback_metrics();
		assert_eq!(drained.queue_depth, before.queue_depth);
		assert_eq!(drained.owned_bytes_current, before.owned_bytes_current);
		assert!(drained.items_drained > before.items_drained);
	}
}
