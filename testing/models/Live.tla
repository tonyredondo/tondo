---- MODULE Live ----
\* A reload that converts values with identity (crates/tondo-runtime/src/live.rs:
\* hold, park, let_go, patch; crates/tondo/src/live.rs: stop_tasks). Every task
\* stops at a statement, the tool sends the patch, and they go on. If the tool
\* goes away the barrier lets go by itself, which `Guarded` makes `patch` see.
CONSTANTS Tasks, Guarded

VARIABLES barrier, task, tool, expired, converted
vars == <<barrier, task, tool, expired, converted>>

Init == /\ barrier = FALSE
        /\ task = [t \in Tasks |-> "runs"]
        /\ tool = "idle"
        /\ expired = FALSE
        /\ converted = FALSE

\* `park`, at a statement: it says it waits, and looks at the barrier.
Statement(t) == /\ task[t] = "runs"
                /\ barrier
                /\ task' = [task EXCEPT ![t] = "parked"]
                /\ UNCHANGED <<barrier, tool, expired, converted>>

\* `hold`: the barrier, and the answer once the tasks settled.
Hold == /\ tool = "idle"
        /\ ~converted
        /\ barrier' = TRUE
        /\ expired' = FALSE
        /\ tool' = "settling"
        /\ UNCHANGED <<task, converted>>

Settled == /\ tool = "settling"
           /\ \A t \in Tasks : task[t] = "parked"
           /\ tool' = "patching"
           /\ UNCHANGED <<barrier, task, expired, converted>>

\* The patch converts what values hold, in place.
Patch == /\ tool = "patching"
         /\ IF Guarded /\ expired
            THEN /\ tool' = "releasing"
                 /\ UNCHANGED converted
            ELSE /\ tool' = "converting"
                 /\ UNCHANGED converted
         /\ UNCHANGED <<barrier, task, expired>>

Convert == /\ tool = "converting"
           /\ converted' = TRUE
           /\ tool' = "releasing"
           /\ UNCHANGED <<barrier, task, expired>>

\* `let_go`.
LetGo == /\ tool = "releasing"
         /\ barrier' = FALSE
         /\ task' = [t \in Tasks |-> "runs"]
         /\ tool' = "idle"
         /\ UNCHANGED <<expired, converted>>

\* After a while with no answer, the barrier lets go by itself.
Expire == /\ barrier
          /\ tool \in {"settling", "patching"}
          /\ barrier' = FALSE
          /\ expired' = TRUE
          /\ task' = [t \in Tasks |-> "runs"]
          /\ UNCHANGED <<tool, converted>>

Over == converted /\ tool = "idle" /\ UNCHANGED vars

Gave == /\ tool = "settling"
        /\ expired
        /\ tool' = "releasing"
        /\ UNCHANGED <<barrier, task, expired, converted>>

Next == \/ \E t \in Tasks : Statement(t)
        \/ Hold \/ Settled \/ Patch \/ Convert \/ LetGo \/ Expire \/ Gave \/ Over

\* While values convert, no task runs.
Quiet == tool = "converting" => \A t \in Tasks : task[t] = "parked"
====
