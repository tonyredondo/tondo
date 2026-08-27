# Native STD-0.1A conformance contract

The stdlib leaf runs the adapter's Core, Hosted and cleanup cases for both
candidate backends. It checks the `Option`/`Result` carrier, capability-gated
host bytes, and resource release against the independent VM observations. A
capability or error-tag drift is a divergence; no partial bytes or backend
specific API is accepted.

The owner matrix is explicit (`std.core` and `std.hosted`) and the target is
`x86_64-unknown-linux-gnu`. This is the native STD-0.1A conformance lane, not a
claim that later STD-0.1B owners or every platform target have shipped.
