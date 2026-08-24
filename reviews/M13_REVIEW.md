# M13 Review — Contact Robustness

Decision: **APPROVED — code-complete / live-unqualified**.

Implemented: feature-aware palm/thumb classification, sticky edge-start
suppression, injected typing suppression, jitter filtering, explicit missing-
feature fallbacks, atomic Arbiter integration, and a CIRQ1080 hardware profile
that records only observed quirks. Generic policy does not depend on CIRQ1080.

Focused evidence: public M13 Arbiter tests passed (5/5), pure robustness tests
passed (5/5), Linux device tests passed, and clippy was clean before later
milestones were layered on top. Final workspace gates are recorded in the M16
review and cover the resulting M13 code transitively.

Qualification boundary: no claim is made that the current hardware exposes
pressure/major/minor/orientation features it does not actually report. Live
palm/thumb/typing behavior remains unqualified.
