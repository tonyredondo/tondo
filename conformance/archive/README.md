# Archived conformance bundles

This directory preserves immutable outputs created by superseded draft tooling.
They are regression evidence only and are never selected by the active CLI or
strict gate.

`candidate-revision-10/` is the last bundle emitted before the distinction
between a promotion-mechanism proof and a normatively complete G5 candidate was
made explicit. Its manifest remains byte-identical and therefore still says
`candidate`; that historical label must not be interpreted as current status.

Current mechanism proofs live under `conformance/proofs/revision-<N>`. A future
normatively complete candidate is created only by `CONF-SEAL-FINAL-001`.
