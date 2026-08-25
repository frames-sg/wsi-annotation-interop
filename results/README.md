# Result retention

The retained schema-v1 result summary predates complete artifact retention. Its manifest may name
fixture, round-trip, or figure files that were not committed, so it is useful historical evidence
but is not an independently retrievable complete study. Other dated result directories are local
run output and are ignored by default.

New schema-v2 runs are published transactionally and CI retains the complete run directory plus a
`.tar.gz` bundle, bundle SHA-256 file, and manifest SHA-256 file. Artifact names use the GitHub run
ID and attempt; the manifest itself records those identifiers and exact source/build provenance.
Publishing an external release asset or permanent URL is an explicit release action and is not
performed by the harness automatically.
