---- MODULE Wake ----
\* How workers find tasks and go to sleep (crates/tondo-runtime/src/tasks/mod.rs:
\* work_until, enqueue, wake_idle, wake_one, step_back). A step here is what the
\* code does with the queue's lock held, or one atomic operation.
EXTENDS Naturals, FiniteSets
CONSTANTS Workers, Main, First, Started, Waits, CEnds

VARIABLES runq, own, nx, parked, cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced
vars == <<runq, own, nx, parked, cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced>>

None == "none"
Idle == {w \in Workers : phase[w] = "asleep"}
Waiting == {w \in Workers : own[w] # {}}
Nexts == {w \in Workers : nx[w] # None}

Init == /\ runq = {First}
        /\ own = [w \in Workers |-> {}]
        /\ nx = [w \in Workers |-> None]
        /\ parked = Waits
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
              /\ UNCHANGED <<runq, nx, parked, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* One turn in 61 its own task goes to the queue, for fairness.
Turn(w) == /\ phase[w] = "look"
           /\ \E t \in own[w] :
                /\ own' = [own EXCEPT ![w] = @ \ {t}]
                /\ runq' = runq \cup {t}
           /\ UNCHANGED <<nx, parked, cur, phase, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* A worker in excess retires, leaving what it had in the queue.
Retire(w) == /\ phase[w] = "look"
             /\ w # Main
             /\ surplus > 0
             /\ surplus' = surplus - 1
             /\ runq' = runq \cup own[w] \cup (IF nx[w] = None THEN {} ELSE {nx[w]})
             /\ own' = [own EXCEPT ![w] = {}]
             /\ nx' = [nx EXCEPT ![w] = None]
             /\ seeking' = IF seeker[w] THEN FALSE ELSE seeking
             /\ seeker' = [seeker EXCEPT ![w] = FALSE]
             /\ phase' = [phase EXCEPT ![w] =
                            IF (runq \cup own[w]) # {} \/ nx[w] # None THEN "retiring" ELSE "spare"]
             /\ UNCHANGED <<parked, cur, timed, waking, todo, replaced>>

\* What it leaves is another's: it wakes one that sleeps. Without this, TLC finds
\* a task in the queue and the last worker asleep, with three workers.
Retiring(w) == /\ phase[w] = "retiring"
               /\ IF Idle # {}
                  THEN \E v \in Idle :
                         /\ phase' = [phase EXCEPT ![w] = "spare", ![v] = "look"]
                         /\ timed' = [timed EXCEPT ![v] = FALSE]
                  ELSE /\ phase' = [phase EXCEPT ![w] = "spare"]
                       /\ UNCHANGED <<timed>>
               /\ UNCHANGED <<nx, parked, runq, own, cur, seeker, seeking, waking, surplus, todo, replaced>>

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
          /\ UNCHANGED <<nx, parked, own, timed, waking, surplus, todo, replaced>>

Steal(w) == /\ phase[w] = "look"
            /\ surplus = 0 \/ w = Main
            /\ runq = {}
            /\ \E v \in Workers \ {w} : \E t \in own[v] :
                 /\ own' = [own EXCEPT ![v] = @ \ {t}]
                 /\ Found(w, t, FALSE)
            /\ UNCHANGED <<nx, parked, runq, timed, waking, surplus, todo, replaced>>

\* Nothing found, or tasks too young to take: it counts itself idle, stops
\* looking, and sleeps; for a moment only if tasks wait and nobody looks.
Sleep(w) == /\ phase[w] = "look"
            /\ runq = {}
            /\ own[w] = {}
            /\ nx[w] = None
            /\ ~(seeker[w] /\ Nexts # {})
            /\ surplus = 0 \/ w = Main
            /\ LET stopped == IF seeker[w] THEN FALSE ELSE seeking
                   again == Waiting # {} /\ ~stopped
               IN /\ seeking' = (stopped \/ again)
                  /\ seeker' = [seeker EXCEPT ![w] = again]
                  /\ timed' = [timed EXCEPT ![w] = again]
            /\ phase' = [phase EXCEPT ![w] = "asleep"]
            /\ UNCHANGED <<nx, parked, runq, own, cur, waking, surplus, todo, replaced>>

Woken(w) == /\ seeker' = [seeker EXCEPT ![w] = @ \/ waking]
            /\ waking' = FALSE
            /\ timed' = [timed EXCEPT ![w] = FALSE]

\* The end of a wait for a moment.
Timeout(w) == /\ phase[w] = "asleep"
              /\ timed[w]
              /\ Woken(w)
              /\ phase' = [phase EXCEPT ![w] = "look"]
              /\ UNCHANGED <<nx, parked, runq, own, cur, seeking, surplus, todo, replaced>>

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
               /\ UNCHANGED <<nx, parked, runq, own, cur, waking, surplus, todo, replaced>>

\* A task starts another: in its worker's queue; the first that waits wakes.
Spawn(w) == /\ phase[w] = "run"
            /\ \E t \in todo :
                 /\ todo' = todo \ {t}
                 /\ own' = [own EXCEPT ![w] = @ \cup {t}]
                 /\ phase' = [phase EXCEPT ![w] = IF own[w] = {} THEN "wakeidle" ELSE "run"]
            /\ UNCHANGED <<nx, parked, runq, cur, timed, seeker, seeking, waking, surplus, replaced>>

\* A task that runs wakes another, which runs next on its worker, and the one
\* there goes to the worker's queue. Waking another worker is rationed.
Notify(w) == /\ phase[w] = "run"
             /\ \E t \in parked :
                  /\ parked' = parked \ {t}
                  /\ nx' = [nx EXCEPT ![w] = t]
                  /\ own' = IF nx[w] = None THEN own ELSE [own EXCEPT ![w] = @ \cup {nx[w]}]
             /\ \/ phase' = [phase EXCEPT ![w] = "wakeidle"]
                \/ UNCHANGED phase
             /\ UNCHANGED <<runq, cur, timed, seeker, seeking, waking, surplus, todo, replaced>>

TakeNext(w) == /\ phase[w] = "look"
               /\ nx[w] # None
               /\ cur' = [cur EXCEPT ![w] = nx[w]]
               /\ nx' = [nx EXCEPT ![w] = None]
               /\ phase' = [phase EXCEPT ![w] = "run"]
               /\ UNCHANGED <<runq, own, parked, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* Only a worker told to look takes the next of another.
TakeOther(w) == /\ phase[w] = "look"
                /\ seeker[w]
                /\ \E v \in Nexts \ {w} :
                     /\ nx' = [nx EXCEPT ![v] = None]
                     /\ Found(w, nx[v], FALSE)
                /\ UNCHANGED <<runq, own, parked, timed, waking, surplus, todo, replaced>>

Finish(w) == /\ phase[w] = "run"
             /\ cur' = [cur EXCEPT ![w] = None]
             /\ phase' = [phase EXCEPT ![w] = "look"]
             /\ UNCHANGED <<nx, parked, runq, own, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* A long call of C, or a task a tool holds: the worker runs nothing else.
EnterC(w) == /\ phase[w] = "run"
             /\ phase' = [phase EXCEPT ![w] = "c"]
             /\ UNCHANGED <<nx, parked, runq, own, cur, timed, seeker, seeking, waking, surplus, todo, replaced>>

\* The monitor starts another when tasks wait and nobody is idle, which is told
\* to look. Without that it would not look at the next of the others, and the
\* task would wait for the call to return, or for ever.
Monitor(w, v) == /\ phase[w] = "c"
                 /\ ~replaced[w]
                 /\ phase[v] = "spare"
                 /\ Idle = {}
                 /\ runq # {} \/ Waiting # {} \/ Nexts # {}
                 /\ replaced' = [replaced EXCEPT ![w] = TRUE]
                 /\ phase' = [phase EXCEPT ![v] = "look"]
                 /\ seeking' = TRUE
                 /\ seeker' = [seeker EXCEPT ![v] = ~seeking]
                 /\ UNCHANGED <<nx, parked, runq, own, cur, timed, waking, surplus, todo>>

\* And one that sleeps is told to look, even if it replaced this one already:
\* the one that did may have taken another task and gone to sleep.
MonitorWakes(w) == /\ phase[w] = "c"
                   /\ Idle # {}
                   /\ ~seeking
                   /\ runq # {} \/ Waiting # {} \/ Nexts # {}
                   /\ \E v \in Idle :
                        /\ seeking' = TRUE
                        /\ seeker' = [seeker EXCEPT ![v] = TRUE]
                        /\ timed' = [timed EXCEPT ![v] = FALSE]
                        /\ phase' = [phase EXCEPT ![v] = "look"]
                   /\ UNCHANGED <<nx, parked, runq, own, cur, waking, surplus, todo, replaced>>

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
                     /\ UNCHANGED <<timed>>
             /\ UNCHANGED <<nx, parked, runq, own, cur, seeker, seeking, waking, todo>>

AllDone == /\ runq = {}
           /\ Nexts = {}
           /\ Waiting = {}
           /\ \A w \in Workers : cur[w] = None \/ (~CEnds /\ phase[w] = "c")
           /\ UNCHANGED vars

Next == \/ \E w \in Workers :
             \/ TakeOwn(w) \/ Turn(w) \/ Retire(w) \/ Retiring(w) \/ Pop(w) \/ Steal(w)
             \/ Sleep(w) \/ Timeout(w) \/ WakeIdle(w) \/ Spawn(w) \/ Finish(w)
             \/ EnterC(w) \/ LeaveC(w) \/ MonitorWakes(w)
             \/ Notify(w) \/ TakeNext(w) \/ TakeOther(w)
             \/ \E v \in Workers : Monitor(w, v)
        \/ AllDone

\* A task runs on one worker at a time.
OneWorker == \A v, w \in Workers : (v # w /\ cur[v] # None) => cur[v] # cur[w]
====
