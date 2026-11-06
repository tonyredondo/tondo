---- MODULE Drain ----
\* A server that stops (crates/tondo-runtime/src/server.rs: accept, handle,
\* tondo_rt_http_server_stop, tondo_rt_http_next; std/http.to: serve, listen).
EXTENDS Naturals, FiniteSets
CONSTANTS Clients

VARIABLES phase, conn, loop, stopper, serving
vars == <<phase, conn, loop, stopper, serving>>

Init == /\ phase = "serving"
        /\ conn = [c \in Clients |-> "none"]
        /\ loop = "listens"
        /\ stopper = "idle"
        /\ serving = "next"

Open == {c \in Clients : conn[c] \in {"counted", "open", "queued", "answering", "answered"}}

\* The loop of `accept`: a connection, or the end once the phase changed. Both
\* may be ready, and it takes either. It counts the connection first and then
\* looks at the phase: without that, TLC finds a connection after the serving
\* loop saw none, whose request nobody answers until the grace ends.
Accept(c) == /\ loop = "listens"
             /\ conn[c] = "none"
             /\ conn' = [conn EXCEPT ![c] = "counted"]
             /\ UNCHANGED <<phase, loop, stopper, serving>>

Looked(c) == /\ conn[c] = "counted"
             /\ conn' = [conn EXCEPT ![c] = IF phase = "serving" THEN "open" ELSE "done"]
             /\ UNCHANGED <<phase, loop, stopper, serving>>

StopListening == /\ loop = "listens"
                 /\ phase # "serving"
                 /\ loop' = "closed"
                 /\ UNCHANGED <<phase, conn, stopper, serving>>

\* A request: to the queue, or refused once closed.
Request(c) == /\ conn[c] = "open"
              /\ IF phase = "closed"
                 THEN conn' = [conn EXCEPT ![c] = "done"]
                 ELSE conn' = [conn EXCEPT ![c] = "queued"]
              /\ UNCHANGED <<phase, loop, stopper, serving>>

\* A connection with nothing to do closes once the server stops.
Close(c) == /\ conn[c] = "open"
            /\ phase # "serving"
            /\ conn' = [conn EXCEPT ![c] = "done"]
            /\ UNCHANGED <<phase, loop, stopper, serving>>

\* `tondo_rt_http_next`: the next request; none once it closed, or drained.
Next1(c) == /\ serving = "next"
            /\ phase # "closed"
            /\ conn[c] = "queued"
            /\ conn' = [conn EXCEPT ![c] = "answering"]
            /\ UNCHANGED <<phase, loop, stopper, serving>>

Answer(c) == /\ conn[c] = "answering"
             /\ conn' = [conn EXCEPT ![c] = "answered"]
             /\ UNCHANGED <<phase, loop, stopper, serving>>

Sent(c) == /\ conn[c] = "answered"
           /\ conn' = [conn EXCEPT ![c] = "done"]
           /\ UNCHANGED <<phase, loop, stopper, serving>>

Ended == /\ serving = "next"
         /\ \/ phase = "closed"
            \/ phase = "draining" /\ Open = {}
         /\ serving' = "ended"
         /\ UNCHANGED <<phase, conn, loop, stopper>>

\* `tondo_rt_http_server_stop`.
Stop == /\ stopper = "idle"
        /\ phase = "serving"
        /\ phase' = "draining"
        /\ stopper' = "waiting"
        /\ UNCHANGED <<conn, loop, serving>>

\* The task that waits for the connections, or for the grace to pass.
Drained == /\ phase = "draining"
           /\ Open = {}
           /\ phase' = "closed"
           /\ UNCHANGED <<conn, loop, stopper, serving>>

Grace == /\ phase = "draining"
         /\ phase' = "closed"
         /\ conn' = [c \in Clients |-> IF conn[c] \in {"open", "queued", "answering", "answered"}
                                       THEN "cut" ELSE conn[c]]
         /\ UNCHANGED <<loop, stopper, serving>>

Returned == /\ stopper = "waiting"
            /\ loop = "closed"
            /\ stopper' = "done"
            /\ UNCHANGED <<phase, conn, loop, serving>>

Over == /\ serving = "ended"
        /\ stopper = "done"
        /\ UNCHANGED vars

Next == \/ \E c \in Clients : Accept(c) \/ Looked(c) \/ Request(c) \/ Close(c) \/ Next1(c) \/ Answer(c) \/ Sent(c)
        \/ StopListening \/ Ended \/ Stop \/ Drained \/ Grace \/ Returned \/ Over

\* The serving loop never ends with a request it did not answer, but past grace.
Answered == serving = "ended" /\ phase = "draining" =>
              \A c \in Clients : conn[c] \notin {"queued", "answering", "answered"}
\* `stop` returned: nothing listens.
Freed == stopper = "done" => loop = "closed"
====
