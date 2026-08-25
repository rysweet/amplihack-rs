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
(*   EnvCeiling   - the depth ceiling is read from the child's environment  *)
(*                  and may be raised by the child (today => TRUE)          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Procs, MaxNodes, MaxDepth, SharedLedger, Locked, EnvCeiling

ASSUME MaxNodes \in Nat /\ MaxNodes > 0
ASSUME MaxDepth \in Nat
ASSUME SharedLedger \in BOOLEAN /\ Locked \in BOOLEAN /\ EnvCeiling \in BOOLEAN

NoProc  == "none"
Ledgers == {"shared"} \cup Procs

VARIABLES
    spawned,    \* spawned[l] : debit count recorded in ledger l
    lock,       \* holder of the flock, or NoProc
    pc,         \* pc[p] \in {"off","live","hold","read","ok","blocked","dead"}
    rdSpawned,  \* rdSpawned[p] : the value p read (its LOCAL copy - TOCTOU)
    depth,      \* depth[p]
    ceiling,    \* ceiling[p] : the max-depth p believes applies to it
    parent,
    child       \* child[p] : the process p is currently admitting, or NoProc

vars == <<spawned, lock, pc, rdSpawned, depth, ceiling, parent, child>>

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
    /\ lock      = NoProc
    /\ pc        = [p \in Procs |-> "off"]
    /\ rdSpawned = [p \in Procs |-> 0]
    /\ depth     = [p \in Procs |-> 0]
    /\ ceiling   = [p \in Procs |-> MaxDepth]
    /\ parent    = [p \in Procs |-> NoProc]
    /\ child     = [p \in Procs |-> NoProc]

(* The root node.  Exactly one, at depth 0, debiting one node. *)
StartRoot(p) ==
    /\ pc[p] = "off"
    /\ \A q \in Procs : pc[q] = "off"   \* exactly one root per tree, ever
    /\ spawned' = [spawned EXCEPT ![Ledger(p)] = @ + 1]
    /\ pc'      = [pc      EXCEPT ![p] = "live"]
    /\ depth'   = [depth   EXCEPT ![p] = 0]
    /\ ceiling' = [ceiling EXCEPT ![p] = MaxDepth]
    /\ UNCHANGED <<lock, rdSpawned, parent, child>>

(* ---- the admission protocol, one file operation per step ---- *)

Acquire(p) ==
    /\ pc[p] = "live"
    /\ \E c \in Procs : pc[c] = "off" /\ c # p
    /\ IF Locked THEN lock = NoProc /\ lock' = p ELSE lock' = lock
    /\ pc' = [pc EXCEPT ![p] = "hold"]
    /\ UNCHANGED <<spawned, rdSpawned, depth, ceiling, parent, child>>

ReadLedger(p) ==
    /\ pc[p] = "hold"
    /\ rdSpawned' = [rdSpawned EXCEPT ![p] = spawned[Ledger(p)]]
    /\ pc'        = [pc        EXCEPT ![p] = "read"]
    /\ UNCHANGED <<spawned, lock, depth, ceiling, parent, child>>

(* Decide using the LOCAL copy - this is where TOCTOU lives when unlocked. *)
Decide(p) ==
    /\ pc[p] = "read"
    /\ \/ /\ rdSpawned[p] + 1 =< MaxNodes
          /\ depth[p] + 1 =< ceiling[p]
          /\ pc' = [pc EXCEPT ![p] = "ok"]
       \/ /\ \/ rdSpawned[p] + 1 > MaxNodes
             \/ depth[p] + 1 > ceiling[p]
          /\ pc' = [pc EXCEPT ![p] = "blocked"]
    /\ UNCHANGED <<spawned, lock, rdSpawned, depth, ceiling, parent, child>>

(* Debit BEFORE the child exists: fail-safe.  A crash here loses capacity
   (reclaimable by a reaper) but can never over-admit. *)
DebitAndSpawn(p, c) ==
    /\ pc[p] = "ok"
    /\ pc[c] = "off"
    /\ c # p
    /\ spawned' = [spawned EXCEPT ![Ledger(p)] = rdSpawned[p] + 1]  \* write-back of the LOCAL copy
    /\ depth'   = [depth   EXCEPT ![c] = depth[p] + 1]
    /\ ceiling' = [ceiling EXCEPT ![c] = ceiling[p]]   \* inherited, never raised
    /\ parent'  = [parent  EXCEPT ![c] = p]
    /\ pc'      = [pc EXCEPT ![p] = "live", ![c] = "live"]
    /\ lock'    = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<rdSpawned, child>>

ReleaseBlocked(p) ==
    /\ pc[p] = "blocked"
    /\ pc'   = [pc EXCEPT ![p] = "live"]
    /\ lock' = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<spawned, rdSpawned, depth, ceiling, parent, child>>

(***************************************************************************)
(* THE OBSERVED FAILURE.  A blocked agent raises its own ceiling and        *)
(* retries one level deeper.  Only reachable when the ceiling is carried    *)
(* in the child's environment rather than read from the ledger.             *)
(*   "retrying investigation-workflow with AMPLIHACK_MAX_DEPTH=8"           *)
(***************************************************************************)
Escalate(p) ==
    /\ EnvCeiling
    /\ pc[p] = "blocked"
    /\ depth[p] + 1 > ceiling[p]
    /\ ceiling' = [ceiling EXCEPT ![p] = @ + 1]
    /\ pc'      = [pc      EXCEPT ![p] = "live"]
    /\ lock'    = IF Locked /\ lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<spawned, rdSpawned, depth, parent, child>>

Complete(p) ==
    /\ pc[p] = "live"
    /\ pc'   = [pc EXCEPT ![p] = "dead"]
    /\ UNCHANGED <<spawned, lock, rdSpawned, depth, ceiling, parent, child>>

(* SIGKILL at any point.  The kernel releases flock on process death. *)
Crash(p) ==
    /\ pc[p] \in {"live","hold","read","ok","blocked"}
    /\ pc'   = [pc EXCEPT ![p] = "dead"]
    /\ lock' = IF lock = p THEN NoProc ELSE lock
    /\ UNCHANGED <<spawned, rdSpawned, depth, ceiling, parent, child>>

Next ==
    \/ \E p \in Procs :
         \/ StartRoot(p) \/ Acquire(p) \/ ReadLedger(p) \/ Decide(p)
         \/ ReleaseBlocked(p) \/ Escalate(p) \/ Complete(p) \/ Crash(p)
    \/ \E p, c \in Procs : DebitAndSpawn(p, c)

Spec == Init /\ [][Next]_vars

(* ------------------------------ properties ------------------------------ *)

(* I3 - tree-global node budget is conserved.  Counts every node ever
   created, which is the quantity that actually consumed 247 GB. *)
NodeBudget == Cardinality({p \in Procs : pc[p] # "off"}) =< MaxNodes

(* I1/I2 - no live node is deeper than the tree ceiling. *)
DepthBound == \A p \in Procs : pc[p] # "off" => depth[p] =< MaxDepth

(* I2 - a child's ceiling never exceeds its parent's. *)
CeilingMonotone ==
    \A c \in Procs : parent[c] \in Procs => ceiling[c] =< ceiling[parent[c]]

(* The ledger never under-counts what exists: no lost update. *)
LedgerSound ==
    SharedLedger =>
        spawned["shared"] >= Cardinality({p \in Procs : pc[p] # "off"})
===============================================================================
