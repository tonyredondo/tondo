# Native generated differential contract

`NATIVE-DIFF-001` closes the deterministic generated differential corpus over
the normalized conformance probe. The generator expands the nine language,
testing and STD-0.1A observations across Cranelift and LLVM, then requires the
same observation IDs, values, tags, diagnostics and cleanup markers as the
independent VM oracle. A changed seed, missing or duplicate case, backend
divergence or physical path is a hard failure.

The normal gate runs this protocol without requiring an LLVM installation, so
it remains a fast and reproducible regression shield. The physical executable
lane is the existing `scripts/native-evaluation-runner.sh`; setting
`TONDO_NATIVE_DIFF_EXECUTABLE=1` runs it with explicit absolute LLVM 18 and C
linker paths and validates its complete native report. The protocol closure is
not a backend selection and does not promote the opt-in lane's measurements;
`NATIVE-001` still owns that decision and the full stress/quality gate.
