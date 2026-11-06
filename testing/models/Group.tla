---- MODULE Group ----
\* A group of tasks (crates/tondo-runtime/src/tasks.rs: tondo_rt_group_add,
\* group_completed, group_try_next, group_block, complete). `add` counts the task
\* with the group's lock, then with the join's lock either finds it done or leaves
\* its place in the join, so that `complete` tells the group.
EXTENDS Naturals, FiniteSets
CONSTANTS Tasks

VARIABLES task, added, known, finished, running, taken, owner, adding, blocked
vars == <<task, added, known, finished, running, taken, owner, adding, blocked>>

Init == /\ task = [t \in Tasks |-> "runs"]
        /\ added = {}
        /\ known = {}
        /\ finished = {}
        /\ running = 0
        /\ taken = {}
        /\ owner = "adding"
        /\ adding = "none"
        /\ blocked = FALSE

\* `add`, with the group's lock.
Add(t) == /\ owner = "adding"
          /\ adding = "none"
          /\ t \notin added
          /\ added' = added \cup {t}
          /\ running' = running + 1
          /\ adding' = t
          /\ UNCHANGED <<task, known, finished, taken, owner, blocked>>

\* Then with the join's lock: done, it tells the group itself.
Link == /\ adding # "none"
        /\ IF task[adding] = "done"
           THEN /\ finished' = finished \cup {adding}
                /\ running' = running - 1
                /\ blocked' = FALSE
                /\ UNCHANGED known
           ELSE /\ known' = known \cup {adding}
                /\ UNCHANGED <<finished, running, blocked>>
        /\ adding' = "none"
        /\ UNCHANGED <<task, added, taken, owner>>

\* `complete`, with the join's lock: a task the group knows tells it.
Complete(t) == /\ task[t] = "runs"
               /\ task' = [task EXCEPT ![t] = "done"]
               /\ IF t \in known
                  THEN /\ finished' = finished \cup {t}
                       /\ running' = running - 1
                       /\ blocked' = FALSE
                  ELSE UNCHANGED <<finished, running, blocked>>
               /\ UNCHANGED <<added, known, taken, owner, adding>>

Added == /\ owner = "adding"
         /\ adding = "none"
         /\ added = Tasks
         /\ owner' = "next"
         /\ UNCHANGED <<task, added, known, finished, running, taken, adding, blocked>>

\* `next`: a result, none when all were taken, or it waits.
Take == /\ owner = "next"
        /\ ~blocked
        /\ IF taken = added
           THEN owner' = "over" /\ UNCHANGED <<taken, blocked>>
           ELSE IF finished \ taken # {}
           THEN /\ \E t \in finished \ taken : taken' = taken \cup {t}
                /\ UNCHANGED <<owner, blocked>>
           ELSE blocked' = TRUE /\ UNCHANGED <<taken, owner>>
        /\ UNCHANGED <<task, added, known, finished, running, adding>>

Over == owner = "over" /\ UNCHANGED vars

Next == \/ \E t \in Tasks : Add(t) \/ Complete(t)
        \/ Link \/ Added \/ Take \/ Over

\* Every result is told once, and the count is right.
Counted == /\ running >= 0
           /\ running + Cardinality(finished) = Cardinality(added)
\* The group never hears of a task that is not done.
Told == \A t \in finished : task[t] = "done"
====
