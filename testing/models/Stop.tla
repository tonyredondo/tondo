---- MODULE Stop ----
\* How the world stops (crates/tondo-runtime/src/tasks/mod.rs: stop_world,
\* resume_world, enter_tondo, leave_tondo). The cycle collector, dumps,
\* profiles, logpoints and reloads each stop it, and read or change memory
\* meanwhile, so nothing may run while any of them is inside.
EXTENDS Naturals, FiniteSets
CONSTANTS Threads, Stoppers

VARIABLES stopping, running, thread, stopper, owner
vars == <<stopping, running, thread, stopper, owner>>

None == "none"

Init == /\ stopping = FALSE
        /\ running = [t \in Threads |-> FALSE]
        /\ thread = [t \in Threads |-> "out"]
        /\ stopper = [s \in Stoppers |-> "idle"]
        /\ owner = None

\* `enter_tondo`: it says it runs, and then looks.
Enter(t) == /\ thread[t] = "out"
            /\ running' = [running EXCEPT ![t] = TRUE]
            /\ thread' = [thread EXCEPT ![t] = "entering"]
            /\ UNCHANGED <<stopping, stopper, owner>>

Look(t) == /\ thread[t] = "entering"
           /\ IF stopping
              THEN /\ running' = [running EXCEPT ![t] = FALSE]
                   /\ thread' = [thread EXCEPT ![t] = "waiting"]
              ELSE /\ thread' = [thread EXCEPT ![t] = "in"]
                   /\ UNCHANGED running
           /\ UNCHANGED <<stopping, stopper, owner>>

\* It waits while the world is stopped, and then enters again.
Resumed(t) == /\ thread[t] = "waiting"
              /\ ~stopping
              /\ thread' = [thread EXCEPT ![t] = "out"]
              /\ UNCHANGED <<stopping, running, stopper, owner>>

\* `leave_tondo`, or a safe point.
Leave(t) == /\ thread[t] = "in"
            /\ running' = [running EXCEPT ![t] = FALSE]
            /\ thread' = [thread EXCEPT ![t] = "out"]
            /\ UNCHANGED <<stopping, stopper, owner>>

\* `stop_world`: one stopper at a time. Without that, TLC finds two inside, and
\* the first to end lets the world go on under the other.
Stop(s) == /\ stopper[s] = "idle"
           /\ owner = None
           /\ stopping' = TRUE
           /\ owner' = s
           /\ stopper' = [stopper EXCEPT ![s] = "stopping"]
           /\ UNCHANGED <<running, thread>>

Stopped(s) == /\ stopper[s] = "stopping"
              /\ \A t \in Threads : ~running[t]
              /\ stopper' = [stopper EXCEPT ![s] = "inside"]
              /\ UNCHANGED <<stopping, running, thread, owner>>

\* It gives up, as when a task runs long without a safe point.
GiveUp(s) == /\ stopper[s] = "stopping"
             /\ stopper' = [stopper EXCEPT ![s] = "resuming"]
             /\ UNCHANGED <<stopping, running, thread, owner>>

Done(s) == /\ stopper[s] = "inside"
           /\ stopper' = [stopper EXCEPT ![s] = "resuming"]
           /\ UNCHANGED <<stopping, running, thread, owner>>

\* `resume_world`.
Resume(s) == /\ stopper[s] = "resuming"
             /\ stopping' = FALSE
             /\ owner' = None
             /\ stopper' = [stopper EXCEPT ![s] = "idle"]
             /\ UNCHANGED <<running, thread>>

Next == \/ \E t \in Threads : Enter(t) \/ Look(t) \/ Resumed(t) \/ Leave(t)
        \/ \E s \in Stoppers : Stop(s) \/ Stopped(s) \/ GiveUp(s) \/ Done(s) \/ Resume(s)

\* While a stopper is inside, no thread runs Tondo code.
Quiet == (\E s \in Stoppers : stopper[s] = "inside") => \A t \in Threads : thread[t] # "in"
\* And no other stopper is inside with it.
Alone == \A s, r \in Stoppers : (s # r /\ stopper[s] = "inside") => stopper[r] # "inside"
====
