# Dogmos IPC transport decision

Status: named-pipe control plane selected for coarse typed commands; fine-grained remote scalar API
rejected. Typed snapshot, lifecycle, adjacency, simulation-stage, and bounded callback-drain
prototype frames pass their current gates. State migration remains gated on repeated idle and
stress captures plus complete service compute and gameplay-event equivalence.

## Decision

Keep the synchronous local named-pipe control plane and do not implement shared memory. The
measured transport is dominated by one wake/wait round trip rather than payload size: sending a
16,388-byte, 1,024-operation batch has nearly the same percentile envelope as an eight-byte scalar
getter. A fixed shared-memory window would therefore add mapped DreamDaemon address space, failure
states, and memory-ordering risk before evidence shows it is needed.

The real DreamDaemon path measured 43.6618-47.7127 microseconds per scalar getter across three
100,000-call repetitions. Therefore a fine-grained remote API is rejected: 1,000 serialized scalar
calls predict 43.7-47.7 ms of wall time before useful work, and 10,000 predict 436.6-477.1 ms. SSair
fires every 0.5 seconds in the reviewed game code. The production API must preserve existing coarse
stage calls and convert only measured read-after-write sequences into typed compound commands.

A valid full-map round-start capture recorded 882,530 detailed calls. A separate settled-idle delta
recorded 635,421 calls across 116 cycles of each coarse SSair stage: about 5,478 calls and 4,694
scalar reads per cycle. Raw remoting predicts 239-261 ms per 500 ms air interval before service
compute, while four coarse stage round trips predict about 0.34 ms at the measured worst p95.
Repeated pressure/temperature/total-moles reads and temporary-mixture register/unregister churn
identify the first snapshot and bulk-lifecycle command families. See
`full-map-operation-pressure.md` for workload caveats and the complete inventory. Repetitions and
stress workloads still gate state migration.

## Tested boundary

- Rust 1.98.0, Windows, release profile.
- i686 `dogmos-byond` benchmark client to x86_64 `dogmosd`.
- `interprocess` 2.4.3 synchronous Windows local named pipe; no Tokio.
- Current-user-only named-pipe DACL plus a 256-bit startup token transferred to the child through
  inherited standard input.
- Protocol/ABI version, raw 20-byte revision, feature and executable digests, PIDs, world
  generation/nonce, and capacity limits verified during the fixed 160-byte handshake.
- One authenticated client; a second client receives a typed busy error.
- Three repetitions of 20,000 measured single-flight operations per message shape after 1,000
  warmups.

Raw ignored evidence is under `tmp/dogmos-perf/ipc/`. The checked-in benchmark entry point is
`tools/benchmark_ipc.ps1`.

## Results

The table reports the range of per-run medians and the worst observed percentile/max across all
three runs.

| Shape | Request / response bytes | p50 range (us) | worst p95 (us) | worst p99 (us) | worst max (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Scalar getter | 8 / 8 | 32.0-40.2 | 84.9 | 127.8 | 12.396 |
| Scalar mutator | 24 / 0 | 28.3-28.4 | 73.8 | 109.5 | 0.748 |
| Two-handle transfer | 24 / 8 | 31.8-33.2 | 75.6 | 124.5 | 0.770 |
| 32-value gas vector | 8 / 260 | 32.4-33.4 | 84.8 | 127.3 | 0.885 |
| Adjacency update | 24 / 0 | 27.7-30.0 | 73.2 | 117.7 | 2.681 |
| One-operation batch | 20 / 4 | 31.8-42.5 | 83.4 | 137.9 | 66.723 |
| 64-operation batch | 1,028 / 4 | 31.2-34.1 | 82.1 | 123.9 | 0.826 |
| 1,024-operation batch | 16,388 / 4 | 36.5-43.2 | 83.0 | 127.2 | 3.475 |
| One callback | 4 / 16 | 31.9-39.4 | 80.4 | 120.4 | 1.575 |
| 64 callbacks | 4 / 1,024 | 30.6-32.1 | 68.7 | 112.1 | 3.627 |
| 1,024 callbacks | 4 / 16,384 | 34.2-39.9 | 58.6 | 118.5 | 12.168 |

The callback rows above are historical protocol-v1 payload-shape measurements from the synthetic
echo server. Protocol v2 replaced that echo with the bounded typed drain described below; do not
interpret the old rows as gameplay callback throughput.

The 66.7 ms maximum is a scheduler outlier and demonstrates why maximum latency remains a failure
and watchdog concern rather than a stable throughput estimator. The p95/p99 distributions are the
appropriate inputs for sequence modeling.

## Typed compound-command prototype

The first workload-derived protocol families use manually encoded, cross-bitness layouts and reject
trailing bytes, count overruns, unknown lifecycle/stage values, and non-finite scalars:

- a fixed 8-byte mixture handle request returning revision, gas count, temperature, volume, and 32
  gas slots in 280 bytes;
- counted 12-byte register/unregister mutations;
- counted 24-byte adjacency mutations containing two handles and a conductivity;
- a fixed 12-byte simulation-stage request containing a typed stage and frame-independent seconds
  per tick.

The protocol validates each counted record before exposing decoded mutations. Malformed wire
payloads receive `InvalidRequest`; valid payloads that conflict with service state receive stable
typed errors such as `UnknownHandle`, `StaleHandle`, or `RevisionMismatch`. Regression tests prove
the authenticated connection remains usable afterward. The first measurements below isolate
protocol and transport cost before service-owned state was enabled.

Three additional release repetitions of 20,000 operations produced:

| Typed shape | Request / response bytes | p50 range (us) | worst p95 (us) | worst p99 (us) | worst max (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| 32-gas mixture snapshot | 8 / 280 | 33.3-34.3 | 84.0 | 114.8 | 4.524 |
| Simulation stage | 12 / 8 | 32.5-34.3 | 86.6 | 130.1 | 2.794 |
| 64 lifecycle mutations | 772 / 4 | 31.3-32.3 | 84.6 | 124.7 | 1.456 |
| 64 adjacency mutations | 1,540 / 4 | 32.4-32.9 | 91.7 | 127.3 | 0.885 |
| 1,024 lifecycle mutations | 12,292 / 4 | 34.3-39.4 | 86.2 | 126.1 | 0.846 |
| 1,024 adjacency mutations | 24,580 / 4 | 35.7-40.2 | 100.2 | 134.6 | 0.911 |

At 64 mutations, one validated batch is roughly 55-62 times cheaper than 64 serialized remote
mutator calls at the corresponding measured median/p95. A mixture snapshot is a boundary/UI
synchronization primitive, not permission to replace every in-service getter with a remote snapshot.
Atmosphere evolution stays behind the four coarse stage barriers.

## Service-owned state and compute prototype

`dogmosd` now owns generational mixture slots, undirected adjacency state, 32-gas mixture vectors,
reusable diffusion input/output buffers, and graph rebuilds. Registration, replacement, unregister,
incident-edge cleanup, stale-handle rejection, unknown-handle rejection, graph degree validation,
snapshot revisioning, and typed stage results are exercised in service tests. Sparse slot growth is
checked against the negotiated service byte budget and uses fallible reservation rather than an
unchecked allocation. This state is still a compatibility spike rather than the complete Auxmos
mixture/reaction implementation.

One exploratory standalone run with real service work measured:

| Service operation | p50 (us) | p95 (us) |
| --- | ---: | ---: |
| 64 lifecycle mutations | 47.3 | 80.1 |
| 64 adjacency mutations with graph validation | 95.6 | 148.7 |
| 1,024 lifecycle mutations | 182.4 | 244.1 |
| 1,024 adjacency mutations with graph validation | 870.5 | 1,072.2 |
| 1,024 mixtures x 32 gases diffusion stage | 182.5 | 254.9 |

The 1,024-mixture stage reuses caller-owned output storage. A simple linear projection from its
single p95 is about 17.5 ms for the 70,214-mixture captured world, but this is only a planning
estimate: topology, active gas count, reactions, heat, callbacks, and full-map contention are not yet
represented.

The repaired protocol-v10 benchmark completed three controlled repetitions on 2026-08-30. It used
Rust 1.98.0, an i686 benchmark client, an x86_64 service, source revision
`7e7b513bae99ebd0b82265b849f70664dfa3bb09`, and feature fingerprint
`7ee0a12425df8b2d85eb47f08d19655a3a217eadec26e9d290da78f6de17bcff`. The working tree contained
the benchmark repair under review, so these measurements qualify that implementation but are not a
clean-revision release result. Each run has a separate success record and exact shim/service PID
memory sample under the ignored `tmp/dogmos-perf/ipc/` directory.

The service case installs 32 gas definitions, registers and seeds 1,024 mixtures, registers a
1,024-turf ring, and atomically commits all turf handles before timing. Every measured stage uses a
new stage epoch and drains pending chunks with the same request. All 1,500 measured logical stages
reported exactly 2,048 work items: 1,024 preparation items plus 1,024 turf-compute items.

| Qualified protocol-v10 case | Iterations per run | p50 range (us) | worst p95 (us) | worst p99 (us) | worst max (ms) |
| --- | ---: | ---: | ---: | ---: | ---: |
| `transport_scalar_getter` | 20,000 | 31.6-32.2 | 84.2 | 124.3 | 3.266 |
| `transport_batch_1024` | 20,000 | 33.5-33.6 | 75.8 | 120.0 | 0.708 |
| `service_mixture_snapshot_32_gases` | 20,000 | 32.5-32.6 | 79.7 | 120.4 | 0.908 |
| `service_simulation_stage_1024_mixtures_32_gases` | 500 | 748.6-777.1 | 949.3 | 1,122.5 | 1.613 |

The stage row measures one complete logical stage across both required chunks; it must not be
combined with the single-round-trip transport rows. The earlier 182.5 us exploratory stage number
used a different, incomplete workload and is not the current protocol-v10 control.

## DreamDaemon call_ext validation

A minimal DreamMaker environment loaded the i686 `dogmos_byond.dll`, launched x86_64 `dogmosd`, and
performed 100,000 synchronous scalar getter calls through generated `load_ext`/`call_ext` bindings.
The harness used a monotonic Rust clock only at the loop boundaries because BYOND's `world.realtime`
did not advance inside the blocking loop.

| Repetition | Calls | Elapsed (us) | Mean per call (us) |
| --- | ---: | ---: | ---: |
| 1 | 100,000 | 4,366,180 | 43.6618 |
| 2 | 100,000 | 4,559,802 | 45.5980 |
| 3 | 100,000 | 4,771,270 | 47.7127 |

These are end-to-end averages including DreamMaker binding dispatch, `call_ext`, shim locking,
encoding, pipe wake/wait, response validation, and value conversion. The standalone distribution
table remains the percentile evidence; this live loop is the call-path validation required to
quantify DreamMaker overhead.

The regenerated fixture then ran each typed prototype for 20,000 calls in three fresh DreamDaemon
processes. The exact i686 shim and x64 service hashes were
`81D58BE95FBBEEDB9C06D2C311A9F76ACF92BAA9842F9732EB37F5E37C84E387` and
`B89224C4359A147AC8434C95ABF4C761FDA596A2095E5A97657601D5E82CD21D`; the DMB hash was
`40490CBBB5A539FF197EC75463A6DFD678C46F1DDB219143D422D6133D87D6BF`.

| Typed call_ext shape | End-to-end mean range (us) |
| --- | ---: |
| 32-gas mixture snapshot | 40.5766-44.2655 |
| 64 lifecycle mutations | 41.4484-43.7751 |
| 64 adjacency mutations | 39.4600-44.3517 |
| Simulation stage | 40.9237-46.3667 |

This confirms that DreamMaker dispatch and response conversion do not erase the batching gain. Four
stage calls predict about 0.164-0.185 ms per SSair cycle at these live means. These repetitions had
no intentionally injected callback backlog and the echo server performed validation rather than
atmosphere compute, so they do not satisfy the callback-pressure or service-compute acceptance gate.

After enabling service-owned state, the fixture registered a 64-mixture ring, read real snapshots,
applied adjacency, and ran 32-gas diffusion for 20,000 stages in each of three fresh DreamDaemon
processes. The tested shim, service, and DMB hashes were
`F8F40F592F4A7F80C417257E0CAC5930DDAF8B01946D8F0D91308200DA823992`,
`3B80C63CB5E45CC9DB0C712E221791E7B4BC4A7204A8AA0AEFCC2B488509D7F4`, and
`E529BC46E87E2860C159958B2EF78E2EDB52F7B1776091898BC588504C19634D`.

| State-backed call_ext shape | End-to-end mean range (us) |
| --- | ---: |
| Scalar transport control | 37.9658-42.5614 |
| 64 lifecycle mutations | 37.7945-42.2441 |
| 32-gas snapshot | 37.1314-43.2904 |
| 64 adjacency mutations | 35.0729-43.1223 |
| 64-mixture x 32-gas diffusion stage | 47.3844-55.7550 |

The actual 64-mixture diffusion work added about 9.4-13.2 microseconds over the same-run scalar
control. These runs validate service compute through the real DreamDaemon boundary, but still lack
representative full-map topology, reactions, heat, and an intentionally saturated bounded callback
queue.

## Bounded service callback pipeline

Protocol v2 replaced the synthetic callback echo with a service-owned `VecDeque` of fixed 40-byte
diagnostic wire events. Protocol v3 now defines a strict 64-byte envelope with four finite numeric
fields and closed gameplay kind/auxiliary enums. The handshake capacity is non-zero and capped at
1,048,576 events. Enqueue is atomic:
if the complete requested batch cannot fit, no event is added, the rejection counter advances, and
the caller receives `CallbackBackpressure`. Each bounded drain returns a 24-byte header containing
returned, remaining, capacity, high-water, and rejected counts, followed by ordered events. The
service writes directly into its reusable control buffer; the i686 fixture uses one fixed 64 KiB
response buffer and returns only a seven-number summary to DreamMaker.

The current service producer remains diagnostic-only. The v3 enum and atomic gameplay-batch state
test establish the wire foundation, but do not prove that reactions, decompression,
pressure-difference effects, turf destruction, or subsystem-cost feedback have migrated. Visual
updates are deliberately absent until their bounded payload is specified.

The following measurements used protocol v2's 40-byte events and are historical process-ownership
evidence. Protocol v3 must repeat this qualification because its saturated service backlog is larger.
Three fresh paired DreamDaemon runs filled a 65,536-event service queue, rejected event 65,537,
sampled both exact PIDs 20 times at 250 ms before and after fill, and drained every event in 41
batches. Every run reported `high_water=65536`, `rejected=1`, `remaining=0`, and a contiguous final
sequence of 65,536.

| Run | DreamDaemon private baseline / saturated / delta (bytes) | dogmosd private baseline / saturated / delta (bytes) |
| --- | ---: | ---: |
| 1 | 3,813,376 / 3,960,832 / +147,456 | 1,921,024 / 4,542,464 / +2,621,440 |
| 2 | 3,801,088 / 3,952,640 / +151,552 | 1,925,120 / 4,542,464 / +2,617,344 |
| 3 | 3,805,184 / 3,956,736 / +151,552 | 1,925,120 / 4,542,464 / +2,617,344 |

DreamDaemon virtual size increased by the same fixed 4,718,592 bytes in all three runs; it did not
grow with the 2.6 MB callback backlog. The tested shim, service, and DMB SHA-256 values were
`8FE1C40DBFD26C675125DDAD60904F5D09D2F312A7F5CD760C9345C11FAB162D`,
`5CF510114034AB87156283BCCFE9DE6289A6AF4D6C3711699171CD5C9C0ED2D9`, and
`A85696CB769EAA3433AB8F9A2D1ACEE8E94839FB22A5DC80B284EA0494B6AF11`. These measurements are
separate process series and are never summed.

Protocol v3 repeated the same fixture in three fresh MCP-owned DreamDaemon processes using a
64-byte event, the fixed 64 KiB shim response buffer, and exact-PID PowerShell samples. Each phase
used 20 samples at 250 ms. All runs accepted 65,536 events, rejected the next event atomically,
drained 65 batches, reported a high water of 65,536 and one rejection, and ended at contiguous
sequence 65,536. Sixty-five batches is expected because the 24-byte header leaves room for 1,023
complete v3 events per response.

| Run | DreamDaemon private baseline / saturated / delta (bytes) | `dogmosd` private baseline / saturated / delta (bytes) |
| --- | ---: | ---: |
| 1 | 3,915,776 / 4,083,712 / +167,936 | 1,957,888 / 6,172,672 / +4,214,784 |
| 2 | 3,911,680 / 4,083,712 / +172,032 | 1,921,024 / 6,123,520 / +4,202,496 |
| 3 | 4,018,176 / 4,186,112 / +167,936 | 1,921,024 / 6,127,616 / +4,206,592 |

DreamDaemon virtual size increased by exactly 4,718,592 bytes in all three v3 runs. The logical
service queue contained 4,194,304 event bytes; its measured private-byte delta was
4,202,496-4,214,784 bytes. Increasing each event from 40 to 64 bytes therefore enlarged the service
backlog while DreamDaemon retained the same fixed transport allocation model. The tested v3 shim,
service, and DMB SHA-256 values were
`5BA02C0450340928558156617EC06BD867F42881751C5448FAF84DCDEC90EF5D`,
`9FB59350977A302F2E9701BD3A6A9B55C665F763DC53B3C6A4B376F314807B12`, and
`A85696CB769EAA3433AB8F9A2D1ACEE8E94839FB22A5DC80B284EA0494B6AF11`.

The same v3 runs measured current end-to-end `call_ext` averages of 68.6203-69.2176 microseconds for
the scalar control, 67.0425-70.3595 for 64 lifecycle mutations, 68.5840-68.9539 for a snapshot,
67.7993-69.4601 for 64 adjacency mutations, and 79.0727-79.9617 for the 64-mixture simulation
stage. These are current-artifact ranges, not a causal comparison against the older v2 build.

Protocol v4 adds a 288-byte fixed-width mixture-state record and counted batch operation. It is
revision-checked and atomic, allowing the service-owned diffusion model to receive nonzero
temperature, volume, and 32-gas state without serial scalar setters. Protocol layout, core atomicity,
same-target service dispatch, and i686-to-x64 state/snapshot round trips are verification gates for
this source revision. The v3 process-memory series above remains the latest saturated-queue memory
evidence until an equivalent v4 callback-pressure run is recorded; do not relabel it as v4 evidence.

Those protocol-v4 functional gates now pass. After the generation-tombstone, revision-exhaustion,
legacy-bound, and typed-error review fixes, one MCP-owned DreamDaemon smoke seeded 64 alternating
nonzero gas states, read the revisioned snapshots, applied the ring adjacency, and completed 20,000
64-mixture stages. End-to-end means were 65.1914 microseconds for the scalar control, 66.8598 for the
64-entry lifecycle batch, 63.8413 for the snapshot, 64.1488 for the 64-edge adjacency batch, and
76.3855 for the nonzero-state stage. It then accepted 65,536 callbacks, rejected the next one,
drained 65 ordered batches through the fixed 64 KiB shim buffer, released the 512 MiB service-only
diagnostic allocation, and shut down cleanly. The tested shim, service, and DMB SHA-256 values were
`53456040F6D9C8F4587E2CE53354EE593776574ACFB1C894DE29284041134AE7`,
`66CDF2455396ABCC49316CAC56277799665BD4F3E7655DC0166FDF000FB18553`, and
`14AF6AC57B15C91450C6EFE14D1D3F1B998C99C47C7D128F685FF5DCEC8F3005`. This is one functional
smoke without paired process sampling, not a latency distribution or protocol-v4 memory result.

A historical protocol-v2 artifact smoke, recorded after adding the hard handshake-capacity ceiling
and callback-sequence exhaustion guard, repeated the 65,536-event fill, typed backpressure,
41-batch drain, and contiguous sequence check through Meridian-MCP. Its shim and service hashes were
`56F97392AB2E3B74CA9FCCBA2B8A0C536E378C6AF1741CD42EBCB3C9C29D3C7F` and
`CE1F349327E43ACB425BAD5295E6EAE1435D245F9FF252EFB662220784E3A62C`; the unchanged DMB hash was
`A85696CB769EAA3433AB8F9A2D1ACEE8E94839FB22A5DC80B284EA0494B6AF11`. It was a functional smoke,
not a fourth paired memory sample.

## Process footprint observation

The standalone i686 benchmark process used 909,312-983,040 private bytes and 17,813,504-19,124,224
virtual bytes at the sampling marker. `dogmosd` used 1,900,544-1,908,736 private bytes and about
4.353 GiB of x64 virtual address space. These are separate series and are not added together.

The i686 process is only a shim stand-in, not DreamDaemon. These samples prove that the control
transport itself is fixed and small; they do not prove the 70% DreamDaemon reduction target.

A separate cross-bitness isolation probe allocated and touched a 536,870,912-byte service arena.
The i686 shim's private bytes did not change; `dogmosd` private bytes grew from 1,900,544 to
539,824,128 bytes, a 537,923,584-byte increase. An initial probe exposed a type-inference bug where
an unannotated diagnostic vector became `Vec<i32>` and consumed four times the requested memory; the
arena is now explicitly `Vec<u8>`.

The real DreamDaemon experiment repeated the same service-only allocation. In two runs,
DreamDaemon private bytes rose by the same fixed 151,552 bytes (3,809,280 to 3,960,832 and 3,776,512
to 3,928,064), while `dogmosd` grew from approximately 1.89 MB to 539.73 MB. DreamDaemon virtual
bytes rose by a fixed 4,718,592 bytes rather than mapping the 512 MiB arena. The service allocation
was released before normal shutdown. DreamDaemon and service samples remain separate and are never
added together.

Stopping the MCP-owned DreamDaemon during an active session also closed the shim's Windows job
object and terminated the exact logged `dogmosd` PID within the five-second assertion window. The
service is launched with `CREATE_NO_WINDOW` and a kill-on-job-close lifetime guard.

## Remaining gate

Repeat the settled-idle capture and add agreed stress workloads with real typed gameplay events.
For each observed
barrier-delimited call sequence, calculate:

`predicted sequence latency = sum(operation count * measured percentile latency)`

Apply the workload-specific SSair allowance and require at least 20% headroom under callback
pressure. Preserve one pipe round trip for existing coarse FDM, equalization, heat, and callback
drain stages. Re-run the live binding with the typed command families and a bounded callback
backlog. Reconsider a fixed shared-memory window only if the coarse command API, not the rejected
scalar API, still fails the measured budget.

The diagnostic binding now embeds the exact 40-character Git revision and the SHA-256 digest of
`dogmos-build-manifest.toml`. The manifest versions the ABI, protocol, and selected root capability
features; tests keep it synchronized with `Cargo.toml` and the protocol constants. Production
identity generation rejects a dirty worktree, while local diagnostic scripts must opt into dirty
builds explicitly. The parent also streams the service executable through Windows CNG SHA-256 before
launch, and `dogmosd` independently hashes its own current executable before creating the pipe; a
parent-supplied mismatch fails before connection acceptance.

`interprocess` 2.4.3 reports Windows named-pipe receive/send timeouts as unsupported. The i686 shim
therefore runs requests on a dedicated I/O worker and bounds the DreamMaker-facing wait with a
channel timeout. On timeout it poisons the channel, calls `CancelIoEx` on a duplicated pipe handle to
cancel the backend's `ReadFileEx`, also calls `CancelSynchronousIo` for a blocked synchronous write,
and the benchmark session drops its kill-on-close job handle before issuing a child-kill fallback.
An i686 test with a deliberately stalled pipe returns `RequestTimeout` inside a 250 ms outer bound
for a 25 ms budget and observes the worker exit inside 500 ms; a normal bounded echo test also passes.

The service interprets nonzero `deadline_ns` as the request's relative processing budget. It rejects
work whose budget is already exhausted and checks the budget every 64 diffusion nodes plus once
before committing output. Cancellation discards scratch output and leaves mixture revisions
unchanged. The independent shim timeout remains authoritative: it cancels the pipe and kills the
service even if native compute fails to reach a checkpoint in time.

This remains diagnostic-only. Production game lifecycle integration, a bounded startup-handshake
path, cancellation checkpoints in each future simulation stage, and repeated DreamDaemon
qualification remain later gates. Populating the wire deadline alone still does not prove a bounded
DreamMaker wait.
