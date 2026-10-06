//! Fresh native Rust process using shared regex fixtures and kernel assertions.
//! This does not execute a regex native ABI or Tondo AOT lowering.

#[path = "../../tondo-stdlib/examples/support/regex_conformance_cases.rs"]
mod cases;

fn main() {
    for id in cases::CASES {
        tondo_native_runtime::tondo_rt_reset();
        let line = cases::run_case(id).expect("registered regex case");
        let live = tondo_native_runtime::tondo_rt_live_objects();
        assert_eq!(live, 0);
        // Case identifiers and computed observables use only ASCII letters,
        // digits, colons and hyphens; this prevents JSON escaping ambiguity.
        assert!(
            line.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-')
        );
        println!(
            r#"{{"id":"{id}","status":"passed","line":"{line}","cleanup":true,"live_runtime_objects":{live}}}"#
        );
    }
    println!(r#"{{"id":"regex-conformance","status":"passed"}}"#);
}
