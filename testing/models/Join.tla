---- MODULE Join ----
\* A task and its join (crates/tondo-runtime/src/tasks/mod.rs: complete, finish,
\* take, cancel_join, tondo_rt_join_detach, end_detached, free_join, parked). A
\* step is what the code does with the join's lock held, or an atomic operation.
EXTENDS Naturals, FiniteSets
CONSTANTS Detaches, Cancels

VARIABLES task, done, detached, rc, listed, flag, waiting, owner, ender, taken, freed, released
vars == <<task, done, detached, rc, listed, flag, waiting, owner, ender, taken, freed, released>>

Init == /\ task = "runs"
        /\ done = FALSE
        /\ detached = FALSE
        /\ rc = 1
        /\ listed = FALSE
        /\ flag = FALSE
        /\ waiting = FALSE
        /\ owner = "has"
        /\ ender = "idle"
        /\ taken = FALSE
        /\ freed = FALSE
        /\ released = 0

\* The task waits, as for a channel; a cancellation ends the wait.
Wait == /\ task = "runs"
        /\ ~flag
        /\ task' = "waits"
        /\ UNCHANGED <<done, detached, rc, listed, flag, waiting, owner, ender, taken, freed, released>>

Wake == /\ task = "waits"
        /\ task' = "runs"
        /\ UNCHANGED <<done, detached, rc, listed, flag, waiting, owner, ender, taken, freed, released>>

\* `complete`, with the lock: it says what was detached, and wakes who waits.
Complete == /\ task = "runs"
            /\ done' = TRUE
            /\ waiting' = FALSE
            /\ task' = IF detached THEN "unlisting" ELSE "ended"
            /\ UNCHANGED <<detached, rc, listed, flag, owner, ender, taken, freed, released>>

\* A detached task leaves the list and lets go of its reference.
Unlist == /\ task = "unlisting"
          /\ listed' = FALSE
          /\ task' = "releasing"
          /\ UNCHANGED <<done, detached, rc, flag, waiting, owner, ender, taken, freed, released>>

TaskRelease == /\ task = "releasing"
               /\ rc' = rc - 1
               /\ task' = IF rc = 1 THEN "freeing" ELSE "ended"
               /\ UNCHANGED <<done, detached, listed, flag, waiting, owner, ender, taken, freed, released>>

\* `tondo_rt_join_detach`.
Detach == /\ Detaches
          /\ owner = "has"
          /\ ~detached
          /\ IF ~done
             THEN /\ detached' = TRUE
                  /\ rc' = rc + 1
                  /\ listed' = TRUE
             ELSE UNCHANGED <<detached, rc, listed>>
          /\ owner' = "releasing"
          /\ UNCHANGED <<task, done, flag, waiting, ender, taken, freed, released>>

\* `job.wait()`: it waits until the task is done, and takes the result.
WaitFor == /\ owner = "has"
           /\ owner' = "waiting"
           /\ waiting' = ~done
           /\ UNCHANGED <<task, done, detached, rc, listed, flag, ender, taken, freed, released>>

Taken == /\ owner = "waiting"
         /\ done
         /\ taken' = TRUE
         /\ owner' = "releasing"
         /\ UNCHANGED <<task, done, detached, rc, listed, flag, waiting, ender, freed, released>>

\* `cancel_join`, with the lock.
Cancel == /\ Cancels
          /\ owner = "has"
          /\ flag' = IF ~done THEN TRUE ELSE flag
          /\ owner' = "waiting"
          /\ waiting' = ~done
          /\ UNCHANGED <<task, done, detached, rc, listed, ender, taken, freed, released>>

\* The owner lets go of its reference.
OwnerRelease == /\ owner = "releasing"
                /\ rc' = rc - 1
                /\ owner' = IF rc = 1 THEN "freeing" ELSE "gone"
                /\ UNCHANGED <<task, done, detached, listed, flag, waiting, ender, taken, freed, released>>

\* `free_join`: it waits for the task, and frees a result nobody took.
Free(p) == /\ done
           /\ ~freed
           /\ freed' = TRUE
           /\ released' = IF taken THEN released ELSE released + 1

OwnerFree == /\ owner = "freeing"
             /\ Free(owner)
             /\ owner' = "gone"
             /\ UNCHANGED <<task, done, detached, rc, listed, flag, waiting, ender, taken>>

TaskFree == /\ task = "freeing"
            /\ Free(task)
            /\ task' = "ended"
            /\ UNCHANGED <<done, detached, rc, listed, flag, waiting, owner, ender, taken>>

\* `end_detached`, when `main` returns: it holds what is listed for a moment.
EndHold == /\ ender = "idle"
           /\ listed
           /\ rc' = rc + 1
           /\ ender' = "cancelling"
           /\ UNCHANGED <<task, done, detached, listed, flag, waiting, owner, taken, freed, released>>

EndCancel == /\ ender = "cancelling"
             /\ flag' = IF ~done THEN TRUE ELSE flag
             /\ ender' = "finishing"
             /\ UNCHANGED <<task, done, detached, rc, listed, waiting, owner, taken, freed, released>>

EndFinish == /\ ender = "finishing"
             /\ done
             /\ rc' = rc - 1
             /\ ender' = IF rc = 1 THEN "freeing" ELSE "idle"
             /\ UNCHANGED <<task, done, detached, listed, flag, waiting, owner, taken, freed, released>>

EndFree == /\ ender = "freeing"
           /\ Free(ender)
           /\ ender' = "idle"
           /\ UNCHANGED <<task, done, detached, rc, listed, flag, waiting, owner, taken>>

Over == /\ task = "ended"
        /\ owner = "gone"
        /\ ender = "idle"
        /\ UNCHANGED vars

Next == \/ Wait \/ Wake \/ Complete \/ Unlist \/ TaskRelease \/ TaskFree
        \/ Detach \/ WaitFor \/ Taken \/ Cancel \/ OwnerRelease \/ OwnerFree
        \/ EndHold \/ EndCancel \/ EndFinish \/ EndFree \/ Over

\* What is freed was done, the result is released once at most, and nothing
\* holds what was freed.
Safe == /\ freed => done
        /\ released <= 1
        /\ ~(taken /\ released > 0)
        /\ rc >= 0
        /\ freed => rc = 0
\* Once every reference is gone the join was freed.
NoLeak == (task = "ended" /\ owner = "gone" /\ ender = "idle") => (rc = 0 /\ freed)
====
