# Grouping Recordings

Captured at full precision by the pinned grouping harness.
Each case retains the original inputs and full expected groups, diagnostics
and identities. The recording run passed all original assertions. Do not regenerate
a case to accommodate an implementation difference.
A case whose contract changed on purpose has its expected `output`, `decisions` and `trace`
replaced, value by value, with what the current grouping returns on the recorded input; the
commit that does so names each case and the rule that changed it.
