# Run and debug

Over `fn main`, *Run* builds and runs the program; *Debug* runs it under
the debugger, with breakpoints, conditions, logpoints and the values of
every frame and task. F5 debugs the program of the file open.

*Tondo: Debug Live* starts a build whose code changes while it runs:
save a file and its new code goes in, keeping the program's state.

The settings `tondo.run.args` and `tondo.run.env` give the program its
arguments and environment; a `launch.json` can give them too.

When the program panics, the debugger stops where it happened.
