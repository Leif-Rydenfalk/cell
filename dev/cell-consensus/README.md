The `cell-consensus` crate adds **Durability** and **Replication** to the Cell ecosystem.

Previously, a Cell was purely functional: it received a signal, processed it, and returned a result. If the Cell crashed or restarted, its memory was wiped.

**`cell-consensus` turns a Cell into a Replicated State Machine.** It allows a Cell to "remember" state changes across restarts and share that state with other copies of itself on the network.

### 1. Durability (The Write-Ahead Log)
It implements a **Write-Ahead Log (WAL)**. Before a Cell changes its internal state (e.g., "Set Key A to Value B"), it **must** write that intent to a file on disk.
*   **How it works:** It serializes the command, calculates a CRC checksum, and appends it to a `.wal` file.
*   **The Benefit:** If the power goes out 1 millisecond after the write, the Cell re-reads the log upon reboot and restores the state exactly as it was.

### 2. Replication (The "Raft" Network)
It creates a dedicated TCP network (separate from the main Cell RPC) to broadcast state changes to peers.
*   **How it works:** When you `propose()` a change, the node writes it to its own disk, applies it, and then immediately streams that binary command to its configured peers.
*   **The Benefit:** You can have 3 instances of a Database Cell running. If one dies, the others still have the data.

### 3. Crash Recovery (Replay)
On startup, before the Cell accepts any new connections, `cell-consensus` reads the WAL from the beginning.
*   **How it works:** It feeds every historical log entry into your `StateMachine` logic.
*   **The Benefit:** The Cell doesn't start "empty"; it starts in the exact state it left off.

### How it fits into the Architecture

It acts as a **Sidecar Library**. You don't have to rewrite your logic to handle files or networking; you just implement the `StateMachine` trait.

**The Data Flow:**

1.  **Incoming Signal:** The Cell receives an RPC (via `Membrane`).
2.  **Proposal:** Instead of updating a HashMap directly, the Cell calls `raft.propose(command)`.
3.  **Consensus Layer:**
    *   Writes command to `run/node-1.wal`.
    *   Sends command to Node 2 and Node 3.
    *   Calls `.apply()` on your logic.
4.  **Response:** The Cell returns success to the user.

### Current Status (MVP)
Right now, it implements a **"Leaderless / All-Write"** consistency model (Simplified Raft):
*   **True Raft:** Only a Leader can write; Followers replicate.
*   **Cell Consensus (Current):** Any node can write; they broadcast to everyone.

**This provides:**
*   ✅ **Atomicity:** Updates happen or they don't.
*   ✅ **Persistence:** Data survives restarts.
*   ✅ **Eventual Consistency:** All nodes eventually get the data.

**It does not yet provide:**
*   ❌ **Strict Linearizability:** If two nodes write to the same key at the exact same microsecond, they might resolve differently (race condition). This requires the full Leader Election logic (Phase 3 of your roadmap).