---- MODULE Chan ----
\* A channel (crates/tondo-runtime/src/tasks.rs: try_send, try_receive,
\* claim_first, close_waits, the waits of send, receive and select). A step is
\* what the code does with the channel's lock held. A wait is claimed by one
\* operation or by a cancellation, as `Wait::claim`, and the claimer notifies.
EXTENDS Naturals, Sequences, FiniteSets
CONSTANTS Senders, Receivers, Capacity, Cancels, Closes

VARIABLES buffer, sending, receiving, closed, sender, receiver, got, cancels
vars == <<buffer, sending, receiving, closed, sender, receiver, got, cancels>>

\* Each sender sends one value, itself.
Values == Senders

Init == /\ buffer = <<>>
        /\ sending = <<>>
        /\ receiving = <<>>
        /\ closed = FALSE
        /\ sender = [s \in Senders |-> "start"]
        /\ receiver = [r \in Receivers |-> "start"]
        /\ got = [r \in Receivers |-> {}]
        /\ cancels = Cancels

Rest(q) == SubSeq(q, 2, Len(q))
Without(q, x) == SelectSeq(q, LAMBDA y : y # x)

\* `send`, with the channel locked.
Send(s) == /\ sender[s] = "start"
           /\ IF closed
              THEN /\ sender' = [sender EXCEPT ![s] = "panicked"]
                   /\ UNCHANGED <<buffer, sending, receiving, receiver, got>>
              ELSE IF receiving # <<>>
              THEN LET r == Head(receiving) IN
                   /\ receiving' = Rest(receiving)
                   /\ got' = [got EXCEPT ![r] = @ \cup {s}]
                   /\ receiver' = [receiver EXCEPT ![r] = "woken"]
                   /\ sender' = [sender EXCEPT ![s] = "sent"]
                   /\ UNCHANGED <<buffer, sending>>
              ELSE IF Len(buffer) < Capacity
              THEN /\ buffer' = Append(buffer, s)
                   /\ sender' = [sender EXCEPT ![s] = "sent"]
                   /\ UNCHANGED <<sending, receiving, receiver, got>>
              ELSE /\ sending' = Append(sending, s)
                   /\ sender' = [sender EXCEPT ![s] = "waiting"]
                   /\ UNCHANGED <<buffer, receiving, receiver, got>>
           /\ UNCHANGED <<closed, cancels>>

\* `receive`, with the channel locked.
Receive(r) == /\ receiver[r] = "start"
              /\ IF buffer # <<>>
                 THEN /\ got' = [got EXCEPT ![r] = @ \cup {Head(buffer)}]
                      /\ IF sending # <<>>
                         THEN /\ buffer' = Append(Rest(buffer), Head(sending))
                              /\ sender' = [sender EXCEPT ![Head(sending)] = "sent"]
                              /\ sending' = Rest(sending)
                         ELSE /\ buffer' = Rest(buffer)
                              /\ UNCHANGED <<sender, sending>>
                      /\ receiver' = [receiver EXCEPT ![r] = "start"]
                      /\ UNCHANGED receiving
                 ELSE IF sending # <<>>
                 THEN /\ got' = [got EXCEPT ![r] = @ \cup {Head(sending)}]
                      /\ sender' = [sender EXCEPT ![Head(sending)] = "sent"]
                      /\ sending' = Rest(sending)
                      /\ receiver' = [receiver EXCEPT ![r] = "start"]
                      /\ UNCHANGED <<buffer, receiving>>
                 ELSE IF closed
                 THEN /\ receiver' = [receiver EXCEPT ![r] = "ended"]
                      /\ UNCHANGED <<buffer, sending, receiving, sender, got>>
                 ELSE /\ receiving' = Append(receiving, r)
                      /\ receiver' = [receiver EXCEPT ![r] = "waiting"]
                      /\ UNCHANGED <<buffer, sending, sender, got>>
              /\ UNCHANGED <<closed, cancels>>

\* A receiver handed a value looks for the next.
Again(r) == /\ receiver[r] = "woken"
            /\ receiver' = [receiver EXCEPT ![r] = "start"]
            /\ UNCHANGED <<buffer, sending, receiving, closed, sender, got, cancels>>

\* `close_waits`: the waits of the channel end, the senders' with a panic.
Close == /\ Closes
         /\ ~closed
         /\ closed' = TRUE
         /\ receiving' = <<>>
         /\ sending' = <<>>
         /\ receiver' = [r \in Receivers |->
                           IF receiver[r] = "waiting" THEN "ended" ELSE receiver[r]]
         /\ sender' = [s \in Senders |->
                         IF sender[s] = "waiting" THEN "panicked" ELSE sender[s]]
         /\ UNCHANGED <<buffer, got, cancels>>

\* A cancellation claims a wait first: the task leaves the queue under the lock.
CancelSender(s) == /\ cancels > 0
                   /\ sender[s] = "waiting"
                   /\ cancels' = cancels - 1
                   /\ sender' = [sender EXCEPT ![s] = "cancelled"]
                   /\ sending' = Without(sending, s)
                   /\ UNCHANGED <<buffer, receiving, closed, receiver, got>>

CancelReceiver(r) == /\ cancels > 0
                     /\ receiver[r] = "waiting"
                     /\ cancels' = cancels - 1
                     /\ receiver' = [receiver EXCEPT ![r] = "cancelled"]
                     /\ receiving' = Without(receiving, r)
                     /\ UNCHANGED <<buffer, sending, closed, sender, got>>

Ended == /\ \A s \in Senders : sender[s] \in {"sent", "panicked", "cancelled", "waiting"}
         /\ \A r \in Receivers : receiver[r] \in {"ended", "cancelled", "waiting"}
         /\ UNCHANGED vars

Next == \/ \E s \in Senders : Send(s) \/ CancelSender(s)
        \/ \E r \in Receivers : Receive(r) \/ Again(r) \/ CancelReceiver(r)
        \/ Close
        \/ Ended

Got == UNION {got[r] : r \in Receivers}

\* A value goes to one receiver.
Once == \A a, b \in Receivers : a # b => got[a] \cap got[b] = {}

\* A value is never lost: what was sent is in the buffer or was received.
Kept == \A s \in Senders :
          sender[s] = "sent" => (s \in Got \/ \E i \in 1..Len(buffer) : buffer[i] = s)

\* A value of a task that gave up was not delivered.
NotBoth == \A s \in Senders : sender[s] \in {"panicked", "cancelled"} => s \notin Got

\* Nobody waits for what is there, nor on a closed channel.
NoUselessWait == /\ (receiving # <<>> => buffer = <<>> /\ sending = <<>>)
                 /\ (closed => receiving = <<>> /\ sending = <<>>)
                 /\ (sending # <<>> => Len(buffer) = Capacity)

Bounded == Len(buffer) <= Capacity
====
