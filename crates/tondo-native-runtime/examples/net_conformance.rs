//! Fresh Rust kernel/model reference process; no native Tondo network provider.

#[path = "../../tondo-stdlib/examples/support/net_conformance_cases.rs"]
mod cases;

fn main() {
    for id in cases::CASES {
        tondo_native_runtime::tondo_rt_reset();
        let observations = cases::kernel(id);
        let live = tondo_native_runtime::tondo_rt_live_objects();
        assert_eq!(live, 0);
        println!(
            "{{\"id\":\"{id}\",\"observations\":[{}],\"live_runtime_objects\":{live}}}",
            cases::encoded_observations(&observations)
        );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fresh_kernel_process_replays_every_common_case() {
        super::main();
    }
}
