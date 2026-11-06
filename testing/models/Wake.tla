---- MODULE Wake ----
\* How workers find tasks and go to sleep (crates/tondo-runtime/src/tasks.rs:
\* work_until, enqueue, wake_idle, wake_one, step_back). A step here is what the
\* code does with the queue's lock held, or one atomic operation.
EXTENDS Naturals, FiniteSets
CONSTANTS Workers, Main, First, Started, CEnds

VARIABLES runq, own, cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced
vars == <<runq, own, cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced>>

None == "none"
Idle == {w \in Workers : phase[w] = "asleep"}
Waiting == {w \in Workers : own[w] # {}}

Init == /\ runq = {First}
        /\ own = [w \in Workers |-> {}]
        /\ cur = [w \in Workers |-> None]
        /\ phase = [w \in Workers |-> IF w = Main THEN "look" ELSE "spare"]
        /\ timed = [w \in Workers |-> FALSE]
        /\ seeker = [w \in Workers |-> FALSE]
        /\ seeking = FALSE
        /\ waking = FALSE
        /\ surplus = 0
        /\ todo = Started
        /\ replaced = [w \in Workers |-> FALSE]

\* A task of its own, which it runs.
TakeOwn(w) == /\ phase[w] = "look"
              /\ \E t \in own[w] :
                   /\ own' = [own EXCEPT ![w] = @ \ {t}]
                   /\ cur' = [cur EXCEPT ![w] = t]
                   /\ phase' = [phase EXCEPT ![w] = "run"]
              /\ UNCHANGED <<runq, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* One turn in 61 its own task goes to the queue, for fairness.
Turn(w) == /\ phase[w] = "look"
           /\ \E t \in own[w] :
                /\ own' = [own EXCEPT ![w] = @ \ {t}]
                /\ runq' = runq \cup {t}
           /\ UNCHANGED <<cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* A worker in excess retires, leaving what it had in the queue.
Retire(w) == /\ phase[w] = "look"
             /\ w # Main
             /\ surplus > 0
             /\ surplus' = surplus - 1
             /\ runq' = runq \cup own[w]
             /\ own' = [own EXCEPT ![w] = {}]
             /\ seeking' = IF seeker[w] THEN FALSE ELSE seeking
             /\ seeker' = [seeker EXCEPT ![w] = FALSE]
             /\ phase' = [phase EXCEPT ![w] =
                            IF (runq \cup own[w]) # {} THEN "retiring" ELSE "spare"]
             /\ UNCHANGED <<cur, timed, waking, todo, replaced>>

\* What it leaves is another's: it wakes one that sleeps. Without this, TLC finds
\* a task in the queue and the last worker asleep, with three workers.
Retiring(w) == /\ phase[w] = "retiring"
               /\ IF Idle # {}
                  THEN \E v \in Idle :
                         /\ phase' = [phase EXCEPT ![w] = "spare", ![v] = "look"]
                         /\ timed' = [timed EXCEPT ![v] = FALSE]
                  ELSE /\ phase' = [phase EXCEPT ![w] = "spare"]
                       /\ UNCHANGED timed
               /\ UNCHANGED <<runq, own, cur, seeker, seeking, waking, surplus, todo, replaced>>

\* Found in the queue, or taken from another worker's: if it was the one
\* looking, it stops, and the next looks if more wait.
Found(w, t, more) ==
    /\ cur' = [cur EXCEPT ![w] = t]
    /\ seeking' = IF seeker[w] THEN FALSE ELSE seeking
    /\ seeker' = [seeker EXCEPT ![w] = FALSE]
    /\ phase' = [phase EXCEPT ![w] = IF more THEN "wakeidle" ELSE "run"]

Pop(w) == /\ phase[w] = "look"
          /\ surplus = 0 \/ w = Main
          /\ \E t \in runq :
               /\ runq' = runq \ {t}
               /\ Found(w, t, (runq \ {t}) # {} \/ (seeker[w] /\ Waiting # {}))
          /\ UNCHANGED <<own, timed, waking, surplus, todo, replaced>>

Steal(w) == /\ phase[w] = "look"
            /\ surplus = 0 \/ w = Main
            /\ runq = {}
            /\ \E v \in Workers \ {w} : \E t \in own[v] :
                 /\ own' = [own EXCEPT ![v] = @ \ {t}]
                 /\ Found(w, t, FALSE)
            /\ UNCHANGED <<runq, timed, waking, surplus, todo, replaced>>

\* Nothing found, or tasks too young to take: it counts itself idle, stops
\* looking, and sleeps; for a moment only if tasks wait and nobody looks.
Sleep(w) == /\ phase[w] = "look"
            /\ runq = {}
            /\ own[w] = {}
            /\ surplus = 0 \/ w = Main
            /\ LET stopped == IF seeker[w] THEN FALSE ELSE seeking
                   again == Waiting # {} /\ ~stopped
               IN /\ seeking' = (stopped \/ again)
                  /\ seeker' = [seeker EXCEPT ![w] = again]
                  /\ timed' = [timed EXCEPT ![w] = again]
            /\ phase' = [phase EXCEPT ![w] = "asleep"]
            /\ UNCHANGED <<runq, own, cur, waking, surplus, todo, replaced>>

Woken(w) == /\ seeker' = [seeker EXCEPT ![w] = @ \/ waking]
            /\ waking' = FALSE
            /\ timed' = [timed EXCEPT ![w] = FALSE]

\* The end of a wait for a moment.
Timeout(w) == /\ phase[w] = "asleep"
              /\ timed[w]
              /\ Woken(w)
              /\ phase' = [phase EXCEPT ![w] = "look"]
              /\ UNCHANGED <<runq, own, cur, seeking, surplus, todo, replaced>>

\* `wake_idle`, whose worker then goes on.
WakeIdle(w) == /\ phase[w] = "wakeidle"
               /\ IF Idle # {} /\ ~seeking
                  THEN \E v \in Idle :
                         /\ seeking' = TRUE
                         /\ seeker' = [seeker EXCEPT ![v] = TRUE]
                         /\ timed' = [timed EXCEPT ![v] = FALSE]
                         /\ phase' = [phase EXCEPT ![w] = "run", ![v] = "look"]
                  ELSE /\ phase' = [phase EXCEPT ![w] = "run"]
                       /\ UNCHANGED <<seeking, seeker, timed>>
               /\ UNCHANGED <<runq, own, cur, waking, surplus, todo, replaced>>

\* A task starts another: in its worker's queue; the first that waits wakes.
Spawn(w) == /\ phase[w] = "run"
            /\ \E t \in todo :
                 /\ todo' = todo \ {t}
                 /\ own' = [own EXCEPT ![w] = @ \cup {t}]
                 /\ phase' = [phase EXCEPT ![w] = IF own[w] = {} THEN "wakeidle" ELSE "run"]
            /\ UNCHANGED <<runq, cur, timed, seeker, seeking, waking, surplus, replaced>>

Finish(w) == /\ phase[w] = "run"
             /\ cur' = [cur EXCEPT ![w] = None]
             /\ phase' = [phase EXCEPT ![w] = "look"]
             /\ UNCHANGED <<runq, own, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* A long call of C, or a task a tool holds: the worker runs nothing else.
EnterC(w) == /\ phase[w] = "run"
             /\ phase' = [phase EXCEPT ![w] = "c"]
             /\ UNCHANGED <<runq, own, cur, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* The monitor starts another when tasks wait and nobody is idle.
Monitor(w, v) == /\ phase[w] = "c"
                 /\ ~replaced[w]
                 /\ phase[v] = "spare"
                 /\ Idle = {}
                 /\ runq # {} \/ Waiting # {}
                 /\ replaced' = [replaced EXCEPT ![w] = TRUE]
                 /\ phase' = [phase EXCEPT ![v] = "look"]
                 /\ UNCHANGED <<runq, own, cur, timed, seeker, seeking, waking, surplus, todo>>

\* The call returns: a worker that was replaced makes one retire, and wakes one.
LeaveC(w) == /\ CEnds
             /\ phase[w] = "c"
             /\ surplus' = IF replaced[w] THEN surplus + 1 ELSE surplus
             /\ replaced' = [replaced EXCEPT ![w] = FALSE]
             /\ IF replaced[w] /\ Idle # {}
                THEN \E v \in Idle :
                       /\ phase' = [phase EXCEPT ![w] = "run", ![v] = "look"]
                       /\ timed' = [timed EXCEPT ![v] = FALSE]
                ELSE /\ phase' = [phase EXCEPT ![w] = "run"]
                     /\ UNCHANGED timed
             /\ UNCHANGED <<runq, own, cur, seeker, seeking, waking, todo>>

AllDone == /\ runq = {}
           /\ Waiting = {}
           /\ \A w \in Workers : cur[w] = None \/ (~CEnds /\ phase[w] = "c")
           /\ UNCHANGED vars

Next == \/ \E w \in Workers :
             \/ TakeOwn(w) \/ Turn(w) \/ Retire(w) \/ Retiring(w) \/ Pop(w) \/ Steal(w)
             \/ Sleep(w) \/ Timeout(w) \/ WakeIdle(w) \/ Spawn(w) \/ Finish(w)
             \/ EnterC(w) \/ LeaveC(w)
             \/ \E v \in Workers : Monitor(w, v)
        \/ AllDone

\* A task runs on one worker at a time.
OneWorker == \A v, w \in Workers : (v # w /\ cur[v] # None) => cur[v] # cur[w]
====
