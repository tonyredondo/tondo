---- MODULE Select ----
\* `select` over two channels, against a task that sends on each and a task
\* that cancels it (crates/tondo-runtime/src/tasks/channels.rs: select, try_case,
\* claim_first; Wait::claim in tasks/mod.rs). The selecting task registers one waiter in each
\* queue under the locks of both, and one `claim` decides which operation won.
\* A sender that loses the claim goes on to the next waiter, or to the buffer.
EXTENDS Naturals, FiniteSets
CONSTANTS Channels, Capacity

VARIABLES buffer, waiter, fired, sel, sender, got, closed, cancelled
vars == <<buffer, waiter, fired, sel, sender, got, closed, cancelled>>

None == "none"

Init == /\ buffer = [c \in Channels |-> 0]
        /\ waiter = [c \in Channels |-> FALSE]
        /\ fired = None
        /\ sel = "start"
        /\ sender = [c \in Channels |-> "start"]
        /\ got = {}
        /\ closed = [c \in Channels |-> FALSE]
        /\ cancelled = FALSE

\* With every lock taken: a channel that is ready, or a waiter in each queue.
Register == /\ sel = "start"
            /\ IF \E c \in Channels : buffer[c] > 0 \/ closed[c]
               THEN \E c \in Channels :
                      /\ buffer[c] > 0 \/ closed[c]
                      /\ got' = IF buffer[c] > 0 THEN got \cup {c} ELSE got
                      /\ buffer' = IF buffer[c] > 0 THEN [buffer EXCEPT ![c] = @ - 1] ELSE buffer
                      /\ sel' = "done"
                      /\ UNCHANGED <<waiter>>
               ELSE /\ waiter' = [c \in Channels |-> TRUE]
                    /\ sel' = "blocked"
                    /\ UNCHANGED <<buffer, got>>
            /\ UNCHANGED <<fired, sender, closed, cancelled>>

\* `try_send`, with the channel's lock: the first waiter it can claim gets the
\* value; one another operation claimed is dropped from the queue.
Send(c) == /\ sender[c] = "start"
           /\ IF closed[c]
              THEN /\ sender' = [sender EXCEPT ![c] = "panicked"]
                   /\ UNCHANGED <<buffer, waiter, fired, got>>
              ELSE IF waiter[c] /\ fired = None
              THEN /\ waiter' = [waiter EXCEPT ![c] = FALSE]
                   /\ fired' = c
                   /\ got' = got \cup {c}
                   /\ sender' = [sender EXCEPT ![c] = "sent"]
                   /\ UNCHANGED buffer
              ELSE IF buffer[c] < Capacity
              THEN /\ waiter' = [waiter EXCEPT ![c] = FALSE]
                   /\ buffer' = [buffer EXCEPT ![c] = @ + 1]
                   /\ sender' = [sender EXCEPT ![c] = "sent"]
                   /\ UNCHANGED <<fired, got>>
              ELSE /\ waiter' = [waiter EXCEPT ![c] = FALSE]
                   /\ sender' = [sender EXCEPT ![c] = "blocked"]
                   /\ UNCHANGED <<buffer, fired, got>>
           /\ UNCHANGED <<sel, closed, cancelled>>

\* `close_waits`.
Close(c) == /\ ~closed[c]
            /\ closed' = [closed EXCEPT ![c] = TRUE]
            /\ IF waiter[c] /\ fired = None
               THEN fired' = c
               ELSE UNCHANGED fired
            /\ waiter' = [waiter EXCEPT ![c] = FALSE]
            /\ sender' = [sender EXCEPT ![c] = IF @ = "blocked" THEN "panicked" ELSE @]
            /\ UNCHANGED <<buffer, sel, got, cancelled>>

\* A cancellation claims the wait like any operation.
Cancel == /\ ~cancelled
          /\ sel = "blocked"
          /\ fired = None
          /\ fired' = "cancelled"
          /\ cancelled' = TRUE
          /\ UNCHANGED <<buffer, waiter, sel, sender, got, closed>>

\* Woken, it leaves every queue with the locks taken.
Leave == /\ sel = "blocked"
         /\ fired # None
         /\ waiter' = [c \in Channels |-> FALSE]
         /\ sel' = "done"
         /\ UNCHANGED <<buffer, fired, sender, got, closed, cancelled>>

Over == /\ sel = "done"
        /\ \A c \in Channels : sender[c] # "start"
        /\ UNCHANGED vars

Next == Register \/ Leave \/ Cancel \/ Over
        \/ \E c \in Channels : Send(c) \/ Close(c)

\* One operation wins, and a value handed over is one the selecting task got.
OneWins == /\ Cardinality(got) <= 1 \/ sel = "start"
           /\ fired \in Channels /\ sel = "done" /\ ~closed[fired] => fired \in got
\* A value is in a buffer, or was got: none is lost.
Kept == \A c \in Channels : sender[c] = "sent" => (c \in got \/ buffer[c] > 0)
\* No waiter outlives the select.
Gone == sel = "done" => \A c \in Channels : ~waiter[c]
====
