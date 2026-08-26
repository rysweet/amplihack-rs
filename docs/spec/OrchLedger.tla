------------------------------ MODULE OrchLedger ------------------------------
(***************************************************************************)
(* Multi-PROCESS admission control for amplihack's nested agent spawning.   *)
(*                                                                          *)
(* The whole point is that this is NOT one process.  Each node is a         *)
(* separate OS process; the only shared state is a file.  So:               *)
(*   - read and write are SEPARATE steps (TOCTOU is expressible)            *)
(*   - a process may be SIGKILLed between any two steps (the OOM killer     *)
(*     took 4,583 processes at once on the affected host)                   *)
(*   - flock is released by the kernel on process death; we model that      *)
(*                                                                          *)
(* Three design decisions are modelled as constants so each can be shown    *)
(* NECESSARY by ablation:                                                   *)
(*   SharedLedger - one ledger per tree, vs. a fresh one per run            *)
(*                  (today: $TMPDIR/amplihack-session-trees with a fresh    *)
(*                   TMPDIR per run => FALSE)                               *)
(*   Locked       - flock around the read-modify-write                      *)
(*   SealAtRoot   - the root writes an authoritative ceiling into the tree  *)
(*                  state, which the environment may only lower.  Agents    *)
(*                  can ALWAYS rewrite their own environment (that is       *)
(*                  reality, not a design choice), so Escalate is always    *)
(*                  enabled; sealing is what makes it inert.                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Procs, MaxNodes, MaxDepth, SharedLedger, Locked, SealAtRoot, Reaping

ASSUME MaxNodes \in Nat /\ MaxNodes > 0
ASSUME MaxDepth \in Nat
ASSUME SharedLedger \in BOOLEAN /\ Locked \in BOOLEAN /\ SealAtRoot \in BOOLEAN
ASSUME Reaping \in BOOLEAN

NoProc  == "none"
Ledgers == {"shared"} \cup Procs

VARIABLES
    spawned,    \* spawned[l] : debit count recorded in ledger l
    holders,    \* holders[l] : procs currently counted as ACTIVE in ledger l
    lock,       \* holder of the flock, or NoProc
    pc,         \* pc[p] \in {"off","live","hold","read","ok","blocked","dead"}
    rdSpawned,  \* rdSpawned[p] : the value p read (its LOCAL copy - TOCTOU)
    depth,      \* depth[p]
    ceiling,    \* ceiling[p] : the max-depth p believes applies to it
    parent,
    child,      \* child[p] : the process p is currently admitting, or NoProc
    isSealed,   \* has the tree recorded an authoritative ceiling?
    sealedVal   \* that ceiling, meaningful only when isSealed

vars == <<spawned, holders, lock, pc, rdSpawned, depth, ceiling, parent, child,
          isSealed, sealedVal>>

Min(a, b) == IF a =< b THEN a ELSE b

(* The ceiling that actually applies to p: the sealed value clamps the value p
   carries in its environment.  Unsealed, the environment is the only source. *)
EffCeiling(p) == IF isSealed THEN Min(sealedVal, ceiling[p]) ELSE ceiling[p]

Live    == {p \in Procs : pc[p] \in {"live","hold","read","ok","blocked"}}
Ledger(p) == IF SharedLedger THEN "shared" ELSE p

TypeOK ==
    /\ spawned  \in [Ledgers -> 0..(MaxNodes + Cardinality(Procs))]
    /\ lock     \in Procs \cup {NoProc}
    /\ pc       \in [Procs -> {"off","live","hold","read","ok","blocked","dead"}]
    /\ depth    \in [Procs -> 0..(MaxDepth + Cardinality(Procs))]
    /\ ceiling  \in [Procs -> 0..(MaxDepth + Cardinality(Procs))]

Init ==
    /\ spawned   = [l \in Ledgers |-> 0]
    /\ holders   = [l \in Ledgers |-> {}]
    /\ lock      = NoProc
    /\ pc        = [p \in Procs |-> "off"]
    /\ rdSpawned = [p \in Procs |-> 0]
    /\ depth     = [p \in Procs |-> 0]
    /\ ceiling   = [p \in Procs |-> MaxDepth]
    /\ parent    = [p \in Procs |-> NoProc]
    /\ child     = [p \in Procs |-> NoProc]
    /\ isSealed  = FALSE
    /\ sealedVal = MaxDepth

(* The root node.  Exactly one, at depth 0, debiting one node. *)
StartRoot(p) ==
    /\ pc[p] = "off"
    /\ \A q \in Procs : pc[q] = "off"   \* exactly one root per tree, ever
    /\ spawned' = [spawned EXCEPT ![Ledger(p)] = @ + 1]
    /\ holders' = [holders EXCEPT ![Ledger(p)] = @ \cup {p}]
    /\ pc'      = [pc      EXCEPT ![p] = "live"]
    /\ depth'   = [depth   EXCEPT ![p] = 0]
    /\ ceiling' = [ceiling EXCEPT ![p] = MaxDepth]
    \* The root either seals the tree's ceiling, or leaves it unsealed.
    /\ isSealed'  = SealAtRoot
    /\ sealedVal' = MaxDepth
    /\ UNCHANGED <<lock, rdSpawned, parent, child>>

(* ---- the admission protocol, one file operation per step ---- *)

Acquire(p) ==
    /\ pc[p] = "live"
    /\ \E c \in Procs : pc[c] = "off" /\ c # p
    /\ IF Locked THEN lock = NoProc /\ lock' = p ELSE lock' = lock
    /\ pc' = [pc EXCEPT ![p] = "hold"]
    /\ UNCHANGED <<spawned, holders, rdSpawned, depth, ceiling, parent, child, isSealed, sealedVal>>

ReadLedger(p) ==
    /\ pc[p] = "hold"
    /\ rdSpawned' = [rdSpawned EXCEPT ![p] = spawned[Ledger(p)]]
    /\ pc'        = [pc        EXCEPT ![p] = "read"]
    /\ UNCHANGED <<spawned, holders, lock, depth, ceiling, parent, child, isSealed, sealedVal>>

(* Decide using the LOCAL copy - this is where TOCTOU lives when unlocked. *)
Decide(p) ==
    /\ pc[p] = "read"
    /\ \/ /\ Cardinality(holders[Ledger(p)]) + 1 =< MaxNodes
          /\ depth[p] + 1 =< EffCeiling(p)
          /\ pc' = [pc EXCEPT ![p] = "ok"]
       \/ /\ \/ Cardinality(holders[Ledger(p)]) + 1 > MaxNodes
             \/ depth[p] + 1 > EffCeiling(p)
          /\ pc' = [pc EXCEPT ![p] = "blocked"]
    /\ UNCHANGED <<spawned, holders, lock, rdSpawned, depth, ceiling, parent, child, isSealed, sealedVal>>

(* Debit BEFORE the child exists: fail-safe.  A crash here loses capacity
   (reclaimable by a reaper) but can never over-admit. *)
DebitAndSpawn(p, c) ==
    /\ pc[p] = "ok"
    /\ pc[c] = "off"
    /\ c # p
    /\ spawned' = [spawned EXCEPT ![Ledger(p)] = rdSpawned[p] + 1]  \* write-back of the LOCAL copy
    /\ holders' = [holders EXCEPT ![Ledger(p)] = @ \cup {c}]
    /\ depth'   = [depth   EXCEPT ![c] = depth[p] + 1]
    /\ ceiling' = [ceiling EXCEPT ![c] = EffCeiling(p)]  \* inherited, never raised
    /\ parent'  = [parent  EXCEPT ![c] = p]
    /\ pc'      = [pc EXCEPT ![p] = "live", ![c] = "live"]
    /\ lock'    = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<rdSpawned, child, isSealed, sealedVal>>

ReleaseBlocked(p) ==
    /\ pc[p] = "blocked"
    /\ pc'   = [pc EXCEPT ![p] = "live"]
    /\ lock' = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<spawned, holders, rdSpawned, depth, ceiling, parent, child, isSealed, sealedVal>>

(***************************************************************************)
(* THE OBSERVED FAILURE.  A blocked agent raises its own ceiling and        *)
(* retries one level deeper.  Only reachable when the ceiling is carried    *)
(* in the child's environment rather than read from the ledger.             *)
(*   "retrying investigation-workflow with AMPLIHACK_MAX_DEPTH=8"           *)
(*                                                                          *)
(* Always enabled: the environment belongs to the agent.  What determines   *)
(* whether it MATTERS is whether a sealed ceiling clamps it.                *)
(***************************************************************************)
Escalate(p) ==
    /\ pc[p] = "blocked"
    /\ depth[p] + 1 > EffCeiling(p)
    \* Bound the ladder so the model stays finite. Real agents escalate a few
    \* times and give up; the observed ladder was 5 -> 6 -> 7 -> 8 -> 9.
    /\ ceiling[p] < MaxDepth + Cardinality(Procs)
    /\ ceiling' = [ceiling EXCEPT ![p] = @ + 1]
    /\ pc'      = [pc      EXCEPT ![p] = "live"]
    /\ lock'    = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<spawned, holders, rdSpawned, depth, parent, child, isSealed, sealedVal>>

(* A clean exit releases the slot: the RAII `Drop` path. *)
Complete(p) ==
    /\ pc[p] = "live"
    /\ pc'      = [pc EXCEPT ![p] = "dead"]
    /\ holders' = [holders EXCEPT ![Ledger(p)] = @ \ {p}]
    /\ UNCHANGED <<spawned, lock, rdSpawned, depth, ceiling, parent, child, isSealed, sealedVal>>

(* SIGKILL at any point.  The kernel releases flock on process death. *)
Crash(p) ==
    /\ pc[p] \in {"live","hold","read","ok","blocked"}
    /\ pc'   = [pc EXCEPT ![p] = "dead"]
    /\ lock' = IF lock = p THEN NoProc ELSE lock
    \* Deliberately does NOT release the slot: `Drop` does not run on SIGKILL.
    /\ UNCHANGED <<spawned, holders, rdSpawned, depth, ceiling, parent, child, isSealed, sealedVal>>

(***************************************************************************)
(* Reclaim a slot whose holder is gone.  Models the pid-liveness check in     *)
(* `admit_session`: an entry whose recorded pid no longer exists is reaped    *)
(* before capacity is judged.  Reaping a LIVE holder would over-admit, which  *)
(* is the failure the budget exists to prevent, so it is guarded on death.    *)
(***************************************************************************)
Reap(a) ==
    /\ Reaping
    /\ pc[a] = "dead"
    /\ a \in holders[Ledger(a)]
    /\ holders' = [holders EXCEPT ![Ledger(a)] = @ \ {a}]
    /\ UNCHANGED <<spawned, lock, pc, rdSpawned, depth, ceiling, parent, child,
                   isSealed, sealedVal>>

Next ==
    \/ \E p \in Procs :
         \/ StartRoot(p) \/ Acquire(p) \/ ReadLedger(p) \/ Decide(p)
         \/ ReleaseBlocked(p) \/ Escalate(p) \/ Complete(p) \/ Crash(p) \/ Reap(p)
    \/ \E p, c \in Procs : DebitAndSpawn(p, c)

Spec == Init /\ [][Next]_vars
          /\ WF_vars(\E a \in Procs : Reap(a))
          /\ WF_vars(\E a \in Procs : Complete(a))

(* ------------------------------ properties ------------------------------ *)

(* I3 - tree-global node budget is conserved.  Counts every node ever
   created, which is the quantity that actually consumed 247 GB. *)
(* The physical claim: at most MaxNodes agent processes exist at once. Splitting
   the ledger per-run does not change how many processes are running -- which is
   exactly why deriving the store from TMPDIR bounded nothing. *)
NodeBudget == Cardinality(Live) =< MaxNodes

(* The bookkeeping claim: no ledger believes it holds more than the budget. *)
AccountingSound == \A l \in Ledgers : Cardinality(holders[l]) =< MaxNodes

(* Reaping never takes a slot from a live holder: guaranteed structurally by
   Reap's `pc[a] = "dead"` guard rather than by an invariant, since a state
   predicate cannot see the transition. *)

(* CapacityRecovers: once every holder is gone, the tree can admit again. Without
   Reap this fails -- which is precisely the SIGKILL leak found in review, and which
   the previous version of this model could not express. *)
CapacityRecovers == (\A p \in Procs : pc[p] \in {"off","dead"})
                      ~> (\A l \in Ledgers : holders[l] = {})

(* I1/I2 - no live node is deeper than the tree ceiling. *)
DepthBound == \A p \in Procs : pc[p] # "off" => depth[p] =< MaxDepth

(* I2 - a child's ceiling never exceeds its parent's. *)
CeilingMonotone ==
    \A c \in Procs : parent[c] \in Procs => EffCeiling(c) =< EffCeiling(parent[c])

(* The ledger never under-counts what exists: no lost update. *)
LedgerSound ==
    SharedLedger =>
        spawned["shared"] >= Cardinality({p \in Procs : pc[p] # "off"})
===============================================================================
