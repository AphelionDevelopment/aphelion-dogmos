# Indexed transaction scratch arena design

## Decision

Proceed with a separate implementation plan for an indexed transaction scratch arena. Do not add
the rewrite to the completed resource-management change set.

After direct topology traversal, a 100,000-turf equalize stage still allocates 157,651-161,935
objects and 195,054,608-196,683,956 bytes. Excited-groups still allocates 214,300-243,281 objects
and 149,154,600-150,413,920 bytes. The remaining work is dominated by ordered maps/sets and cloned
`MixtureRecord` rollback state, not vector growth. Retaining completed stage vectors would therefore
pin memory without addressing the leading cost.

## Required semantics

- Preserve sorted turf/component discovery and exact callback order.
- Keep cancellation, event-capacity failure, revision exhaustion, duplicate mutable-mixture
  rejection, and topology revision conflicts atomic.
- Never expose staged records through snapshots or later components before the complete stage
  commits.
- Preserve immutable mixtures and the current equalize hard turf limit.
- Bound arena memory through the negotiated world budget and use fallible reservation.

## Proposed representation

Create one stage-local arena indexed by dense mixture slot. It owns:

- a bitset marking mixtures touched by the transaction;
- a dense `Vec<MixtureRecord>` containing one original record per touched mixture;
- a parallel dense `Vec<MixtureRecord>` containing the candidate record;
- a slot-to-dense-index vector using a sentinel for untouched slots;
- ordered staged events; and
- component scratch vectors for turf slots, parents, balances, and flows.

The slot-to-index vector replaces repeated `BTreeMap<MixtureHandle, MixtureRecord>` lookups. A
generation check occurs before the first insertion. Later accesses use the dense index only after
verifying the stored handle, so ABA protection remains explicit.

## Transaction flow

1. Reserve bounded scratch before mutating any candidate record.
2. On first touch, copy the authoritative record into both original and candidate arrays and record
   its handle/index.
3. Run component math only against candidate records. Append events to the transaction-owned event
   vector.
4. On cancellation or any error, clear logical arena lengths. Authoritative world records and the
   public event queue remain untouched.
5. Before commit, revalidate every handle/revision and the combined event capacity.
6. Commit candidates in deterministic handle order, incrementing revisions exactly once, then append
   events in their staged order.

Equalization endpoint updates should borrow two disjoint candidate records through a checked
split-at helper. This removes the current clone-update-insert cycle without unsafe aliasing.

## Delivery slices

1. Add arena-only unit tests for first touch, repeated touch, ABA rejection, fallible capacity, and
   disjoint two-record access.
2. Port equalize candidate records while retaining existing component discovery and event code.
3. Port excited-groups only after equalize transcript and rollback equivalence passes.
4. Re-run the allocation probe in three fresh processes. Accept only with identical transcript
   hashes, at least 50% lower allocated bytes for the selected stage, and no increase in retained
   idle service memory beyond the explicitly budgeted arena capacity.
5. Run the full i686 workspace, x64 core/server, legal IPC, and external paired-game gates.

## Rejected shortcuts

- Reusing only the existing stage vectors does not address per-entry tree allocation.
- Mutating authoritative records and restoring on error risks exposing partial state and increases
  rollback work.
- An unsafe multi-record accessor is unnecessary; checked dense indices plus `split_at_mut` provide
  disjoint access.
