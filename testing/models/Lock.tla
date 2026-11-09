---- MODULE Lock ----
\* A lock of `std.sync` (crates/tondo-runtime/src/sync.rs: try_take, taken, grant,
\* wake_lockers, tondo_rt_sync_unlock). A step is an atomic operation on the
\* state, or what the code does with the queue locked.
EXTENDS Naturals, Sequences, FiniteSets
CONSTANTS Writers, Readers, Rounds, Cancels

Tasks == Writers \cup Readers
Exclusive(t) == t \in Writers

VARIABLES writer, readers, parked, starving, waiting, pc, woken, rounds, cancels, was
vars == <<writer, readers, parked, starving, waiting, pc, woken, rounds, cancels, was>>

Init == /\ writer = FALSE
        /\ readers = 0
        /\ parked = FALSE
        /\ starving = FALSE
        /\ waiting = <<>>
        /\ pc = [t \in Tasks |-> "idle"]
        /\ woken = [t \in Tasks |-> FALSE]
        /\ rounds = [t \in Tasks |-> Rounds]
        /\ cancels = Cancels
        /\ was = [t \in Tasks |-> FALSE]

Rest(q) == SubSeq(q, 2, Len(q))
Without(q, x) == SelectSeq(q, LAMBDA y : y # x)

\* `try_take`.
Free(t) == IF Exclusive(t) THEN ~writer /\ readers = 0 /\ ~starving
           ELSE ~writer /\ ~starving
Take(t) == IF Exclusive(t) THEN writer' = TRUE /\ UNCHANGED readers
           ELSE readers' = readers + 1 /\ UNCHANGED writer

Start(t) == /\ pc[t] = "idle"
            /\ rounds[t] > 0
            /\ rounds' = [rounds EXCEPT ![t] = @ - 1]
            /\ pc' = [pc EXCEPT ![t] = "try"]
            /\ woken' = [woken EXCEPT ![t] = FALSE]
            /\ UNCHANGED <<writer, readers, parked, starving, waiting, cancels, was>>

Try(t) == /\ pc[t] = "try"
          /\ IF Free(t)
             THEN /\ Take(t)
                  /\ pc' = [pc EXCEPT ![t] = "holds"]
             ELSE /\ pc' = [pc EXCEPT ![t] = "park"]
                  /\ UNCHANGED <<writer, readers>>
          /\ UNCHANGED <<parked, starving, waiting, woken, rounds, cancels, was>>

\* With the queue locked: PARKED, a last look, and into the queue; one woken
\* before goes first, and may make the lock starve.
Park(t) == /\ pc[t] = "park"
           /\ IF Free(t)
              THEN /\ Take(t)
                   /\ parked' = TRUE
                   /\ pc' = [pc EXCEPT ![t] = "holds"]
                   /\ UNCHANGED <<starving, waiting>>
              ELSE /\ parked' = TRUE
                   /\ pc' = [pc EXCEPT ![t] = "waits"]
                   /\ waiting' = IF woken[t] THEN <<t>> \o waiting ELSE Append(waiting, t)
                   /\ \/ starving' = starving
                      \/ woken[t] /\ starving' = TRUE
                   /\ UNCHANGED <<writer, readers>>
           /\ UNCHANGED <<woken, rounds, cancels, was>>

\* The release: the state first, and `before` says if anyone is parked.
Release(t) == /\ pc[t] = "holds"
              /\ IF Exclusive(t) THEN writer' = FALSE /\ UNCHANGED readers
                 ELSE readers' = readers - 1 /\ UNCHANGED writer
              /\ was' = [was EXCEPT ![t] = parked]
              /\ pc' = [pc EXCEPT ![t] = "wake"]
              /\ UNCHANGED <<parked, starving, waiting, woken, rounds, cancels>>

CanGrant(t) == IF Exclusive(t) THEN ~writer /\ readers = 0 ELSE ~writer

\* `wake_lockers`, with the queue locked, in its two ways, for a writer or the
\* first reader at the head of the queue (a wait a cancellation claimed has left
\* it or is skipped: see Cancel).
Wake(t) == /\ pc[t] = "wake"
           /\ IF ~was[t]
              THEN /\ pc' = [pc EXCEPT ![t] = "idle"]
                   /\ UNCHANGED <<writer, readers, parked, starving, waiting, woken>>
              ELSE IF waiting = <<>>
              THEN /\ pc' = [pc EXCEPT ![t] = "idle"]
                   /\ starving' = FALSE
                   /\ parked' = FALSE
                   /\ UNCHANGED <<writer, readers, waiting, woken>>
              ELSE LET h == Head(waiting) IN
                   IF starving
                   THEN IF CanGrant(h)
                        THEN /\ Take(h)
                             /\ waiting' = Rest(waiting)
                             /\ pc' = [pc EXCEPT ![t] = "idle", ![h] = "holds"]
                             /\ starving' = (Rest(waiting) # <<>>)
                             /\ parked' = (Rest(waiting) # <<>>)
                             /\ UNCHANGED woken
                        ELSE /\ pc' = [pc EXCEPT ![t] = "idle"]
                             /\ UNCHANGED <<writer, readers, parked, starving, waiting, woken>>
                   ELSE /\ waiting' = Rest(waiting)
                        /\ pc' = [pc EXCEPT ![t] = "idle", ![h] = "try"]
                        /\ woken' = [woken EXCEPT ![h] = TRUE]
                        /\ parked' = (Rest(waiting) # <<>>)
                        /\ UNCHANGED <<writer, readers, starving>>
           /\ UNCHANGED <<rounds, cancels, was>>

\* A cancellation claims a wait, and the task leaves the queue. A release that
\* finds it first only passes it by, so this is the same step.
Cancel(t) == /\ pc[t] = "waits"
             /\ cancels > 0
             /\ cancels' = cancels - 1
             /\ waiting' = Without(waiting, t)
             /\ pc' = [pc EXCEPT ![t] = "idle"]
             /\ UNCHANGED <<writer, readers, parked, starving, woken, rounds, was>>

Over == /\ \A t \in Tasks : pc[t] = "idle" /\ rounds[t] = 0
        /\ UNCHANGED vars

Next == \/ \E t \in Tasks : Start(t) \/ Try(t) \/ Park(t) \/ Release(t) \/ Wake(t) \/ Cancel(t)
        \/ Over

\* One writer alone, or readers.
Exclusion == /\ Cardinality({t \in Writers : pc[t] = "holds"}) <= 1
             /\ (\E t \in Writers : pc[t] = "holds") => ~\E t \in Readers : pc[t] = "holds"
             /\ writer => readers = 0
\* A task is in the queue only while it waits there: the wait it waited with
\* is its own again for its next (`Wait::new`), as no release can claim it.
Gone == \A i \in 1..Len(waiting) : pc[waiting[i]] = "waits"
====
