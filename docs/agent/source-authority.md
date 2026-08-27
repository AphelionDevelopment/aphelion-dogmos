# Source authority and lineage

Reviewed local revision: `e0405c0398aba8851dca507cfcd27d4a898dff4c`

Reviewed Auxmos revision: `7757b8eb677796fc3b184768cfe83e91f5b92cba`

Reviewed on: `2026-08-26`

| Question | Authority |
| --- | --- |
| Dogmos product behavior and target ownership | This repository's checked-in guidance, tests, and implementation. |
| DM-facing contract and game scheduling | The paired Meridian-Rift `dogmos` implementation and generated bindings. |
| Original Auxmos intent | Putnam3145/auxmos at the reviewed revision, used as historical evidence rather than current Dogmos policy. |
| BYOND ABI behavior | Official BYOND behavior plus a real i686 native-load compile/boot test. |
| Rust dependency behavior | The exact revisions in `Cargo.lock` and their primary documentation/source. |
| Numerical correctness | Explicit invariants, differential fixtures, and reproducible tests. |
| Performance | Repeated identical workloads with separate DreamDaemon and service measurements. |

The reviewed local revision is a source baseline, so the checker requires it to be an ancestor of current `HEAD`. Requiring a document to contain the hash of the commit that contains the document would be self-referential. Update this anchor after reviewing a new baseline, not after every unrelated commit.

When sources disagree, identify the contract in question. Preserve an established Dogmos gameplay contract unless a test and product decision change it. Treat Auxmos code as provenance, not a reason to restore removed or feature-disabled behavior automatically. A host-only build, parser result, or generated file does not override a failing i686/DM integration gate.
