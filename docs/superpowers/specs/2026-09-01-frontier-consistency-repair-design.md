# Frontier Consistency Repair Design

## Objective

Prevent a rejected or malformed incremental frontier mutation from advancing DreamMaker's published frontier state, prevent mixture teardown from changing service topology during a resumable simulation stage, and stop further native calls after SSair enters its fatal fail-closed state.

## Confirmed failures

The 2026-09-01 playtest admitted an active turf with a null Dogmos registration generation. DreamMaker continued after `CRASH()`, encoded the null value, advanced its local frontier epoch, and published epoch 1126 even though `dogmosd` remained at 1125. Stage 4 then failed with `StageConflict` and correctly triggered the controlled reboot.

An earlier playtest failed with a topology revision changing during a resumable stage. All explicit turf topology mutators reject active-stage changes, but mixture lifecycle teardown can detach turfs and remove incident topology without the same guard.

## Design

DreamMaker will build and validate every incremental frontier pair before performing topology flushes or frontier mutations. Missing active-turf registration is repaired through the normal turf registration path; a turf that remains invalid is reported with its type, coordinates, atmosphere state, and mixture identities, then SSair fails closed without publishing a new epoch.

Incremental frontier chunks use a candidate epoch. The candidate becomes DreamMaker's committed epoch only after a fixed-width response confirms service acceptance. Callers update the committed turf map only after every required chunk succeeds. A rejected chunk returns failure explicitly instead of relying on `CRASH()` to stop execution.

The native world rejects any mixture lifecycle batch while a simulation stage is active. This closes the only identified path that can detach turf mixtures and mutate topology without the same stage barrier as turf lifecycle, heat, adjacency, and firelock changes.

SSair latches fatal Dogmos failure state before scheduling its controlled reboot. Dogmos entry points return without another FFI request while that latch is set. The original service diagnostic remains authoritative; shutdown-time callers do not create an error avalanche or attempt to mutate a failed service.

## Verification

Focused Rust and DM tests must be observed failing before implementation. Native verification covers the locked x86_64 core/server tests and i686 BYOND-facing workspace gates. Meridian verification covers the focused Dogmos tests, DreamMaker compilation, paired native contract, full-map boot, and applicable wider suite. Generated bindings and native artifacts are regenerated and installed only through maintained tooling.
