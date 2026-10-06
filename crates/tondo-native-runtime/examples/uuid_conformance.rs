//! Fresh Rust-kernel process; no native UUID ABI or Tondo AOT execution.

#[path = "../../tondo-stdlib/examples/support/uuid_conformance_cases.rs"]
mod cases;

fn main() {
    for id in cases::CASES {
        tondo_native_runtime::tondo_rt_reset();
        let observations = cases::run_kernel_case(id);
        let live = tondo_native_runtime::tondo_rt_live_objects();
        assert_eq!(live, 0);
        // These observations contain only canonical UUIDs, tags and integers.
        let encoded = observations
            .iter()
            .map(|line| {
                assert!(
                    line.bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b":|- ".contains(&byte))
                );
                format!("\"{line}\"")
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"id\":\"{id}\",\"observations\":[{encoded}],\"live_runtime_objects\":{live}}}"
        );
    }
}
