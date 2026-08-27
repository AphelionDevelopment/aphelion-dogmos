# Upstream drift review

Review Auxmos, byondapi, BYOND compatibility, Rust dependencies, and the paired Meridian-Rift contract when an upstream is intentionally updated or at least monthly while active development continues. Automation reports drift; it does not rewrite policy or float production revisions.

Record:

```text
Reviewed on:
Reviewer:
Local HEAD and reviewed baseline:
Auxmos revision and relevant paths:
byondapi revision and ABI changes:
BYOND versions exercised:
Meridian-Rift revision and binding digest:
Changed authoritative behavior:
Retained Dogmos deltas and reasons:
Removed/dead upstream features that remain unsupported:
Follow-up issues:
```

Compare behavior and invariants, not filenames alone. Auxmos is historical source material; do not restore Monstermos, Putnamos, alternate reaction backends, legacy callbacks, or old allocation patterns solely because upstream contains them. Preserve the default Dogmos feature contract unless a separately reviewed change updates code, tests, generated bindings, artifacts, and the game together.

For byondapi changes, review exported wrapper generation, panic behavior, thread/main-thread constraints, target support, and a real native-load boot. For numerical upstream changes, require differential/property tests and repeated workload evidence before adopting them.

Update [source authority](source-authority.md) only after the referenced source was actually reviewed. A remote branch name or latest release label is not a revision anchor.
