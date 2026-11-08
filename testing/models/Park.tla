---- MODULE Park ----
\* How a task waits (crates/tondo-runtime/src/tasks/mod.rs: park, run, notify_with).
\* It looks at what it waits for, parks, and a worker finishes parking it once
\* it is off its stack, while another task may notify it at any of those steps.
EXTENDS Naturals
CONSTANTS Notifies

VARIABLES state, pc, event, left, queued
vars == <<state, pc, event, left, queued>>

Init == /\ state = "running"
        /\ pc = "check"
        /\ event = FALSE
        /\ left = Notifies
        /\ queued = FALSE

\* What it waits for came: it is done. Otherwise it goes to park.
Check == /\ pc = "check"
         /\ pc' = IF event THEN "done" ELSE "park"
         /\ UNCHANGED <<state, event, left, queued>>

\* `park`: RUNNING to PARKING, or a notification came and it looks again.
Park == /\ pc = "park"
        /\ IF state = "running"
           THEN /\ state' = "parking"
                /\ pc' = "out"
           ELSE /\ state' = "running"
                /\ pc' = "check"
        /\ UNCHANGED <<event, left, queued>>

\* `run`, on its worker: PARKING to PARKED, or notified meanwhile, and it runs.
Out == /\ pc = "out"
       /\ IF state = "parking"
          THEN /\ state' = "parked"
               /\ pc' = "parked"
               /\ UNCHANGED queued
          ELSE /\ state' = "running"
               /\ pc' = "queued"
               /\ queued' = TRUE
       /\ UNCHANGED <<event, left>>

\* A worker takes it from the queue.
Resume == /\ pc \in {"parked", "queued"}
          /\ queued
          /\ queued' = FALSE
          /\ pc' = "check"
          /\ UNCHANGED <<state, event, left>>

\* `notify_with`, after what it waits for came.
Notify == /\ left > 0
          /\ left' = left - 1
          /\ event' = TRUE
          /\ CASE state = "parked" -> state' = "running" /\ queued' = TRUE
             []   state = "notified" -> UNCHANGED <<state, queued>>
             []   OTHER -> state' = "notified" /\ UNCHANGED queued
          /\ UNCHANGED pc

Finished == pc = "done" /\ UNCHANGED vars

Next == Check \/ Park \/ Out \/ Resume \/ Notify \/ Finished

\* A task is in the queue only once it can run.
Queued == queued => state \in {"running", "notified"}
\* Parked with what it waits for there, and nobody to wake it, is a lost wake.
NoLostWake == ~(pc = "parked" /\ state = "parked" /\ event /\ left = 0)
====
