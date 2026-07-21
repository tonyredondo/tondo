# Runtime fixtures

A `.to` case compiles and executes through the verified VM. `.stdout`,
`.runtime-stderr`, and `.exit` sidecars describe exact program results;
`.stderr` remains the human compiler-diagnostic snapshot. Runtime cases become
active when M3 provides bytecode execution.
