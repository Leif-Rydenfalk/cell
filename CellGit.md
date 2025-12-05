Path: /Users/07lead01/cell/CellGit.md
```markdown
# Mycelial Distribution: The Decentralized Registry

**"Code is a spore. It floats in the network. If it lands on fertile silicon, it grows."**

Traditional package managers (npm, crates.io, the previous design of cell-git) rely on a central cathedral. If the cathedral burns down, the ecosystem dies.

**Cell takes a biological approach.** There is no central registry. There is no `cell.network` API server. Every running Cell instance is a potential seeder for the source code it runs.

## 1. The Core Shift: Identity, not Location

In a decentralized system, we do not ask "Where is the code?" (URL). We ask "Who signed the code?" (Identity).

### The New Cell.toml
We replace Git URLs with **Cryptographic Identities**.

```toml
[cell]
# OLD (Centralized/Location-based):
# exchange = { git = "https://github.com/acme/exchange", tag = "v1.0.0" }

# NEW (Decentralized/Identity-based):
exchange = { 
    # The Ed25519 Public Key of the author/organization
    authority = "ed25519:7a8f3b2c9d1e...", 
    
    # The name of the cell within their namespace
    name = "exchange",
    
    # The signed Git tag to resolve
    version = "1.0.0",
    
    # Optional: Bootstrap hints (if DHT fails, try these locations)
    # This provides a bridge to the old world without relying on it.
    mirrors = ["https://github.com/acme/exchange"]
}
```

## 2. Architecture: The Global DHT

We utilize a **Kademlia Distributed Hash Table (DHT)** embedded directly into the `cell-sdk`.

*   **Network Layer:** Axon (QUIC/UDP).
*   **Key:** `SHA256(Authority_PublicKey + Cell_Name)`.
*   **Value:** A list of IP addresses (Peers) currently seeding this repository.

### Bootstrap Nodes (Inoculation)
We still need an entry point to join the swarm. These are **Stateless Introducers**. They store no code. They only store the DHT routing table.
*   `seed.cell.network`
*   `bootstrap.community.rs`
*   `192.168.1.5` (Local LAN bootstrap)

## 3. The Lifecycle of a Spore

### Phase A: Publishing (Sporulation)
You don't "push" to a server. You sign locally.
1.  Developer commits code to local git repo.
2.  Developer runs `cell sign v1.0.0`.
3.  The local SDK signs the git tag with the private key.
4.  The local SDK announces to the DHT: *"I am seeding `authority:name`."*

### Phase B: Resolution (Germination)
A consumer builds a project depending on `exchange`.
1.  **Lookups:** Build script calculates the DHT Key.
2.  **Discovery:** Queries the swarm via Axon. Finds 12 peers.
3.  **Transport:** Connects to the fastest peer via QUIC.
4.  **Transfer:** Tunnels `git-upload-pack` over the QUIC stream.
5.  **Verification (Immune Response):**
    *   Download completes.
    *   SDK verifies the GPG/Ed25519 signature of the tag against the `authority` key in `Cell.toml`.
    *   **Signature Mismatch = Build Error.** (Prevents poisoning).

### Phase C: Seeding (Symbiosis)
Once a consumer has downloaded and compiled the cell:
1.  It holds the cache in `~/.cell/cache/repos/`.
2.  **It automatically joins the swarm.**
3.  It announces to the DHT that it also has this code.
*   **Result:** Popular cells become highly available naturally. Unused cells fade away.

## 4. Implementation Strategy

We do not build a separate binary. We extend `cell-sdk`.

### 4.1. The Git-Over-Axon Protocol
We define a custom ALPN protocol for QUIC: `cell-git/1`.

**In `cell-sdk/src/axon.rs`:**
```rust
// Pseudo-code for handling incoming git fetches
async fn handle_git_stream(stream: QuicStream) {
    let repo_id = read_handshake(stream).await;
    
    // Check if we have this repo cached
    if let Some(path) = cache.get(repo_id) {
        // Pipe standard git-upload-pack to the QUIC stream
        let mut child = Command::new("git-upload-pack")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
            
        // Zero-copy splice (Linux) or copy loop
        tokio::io::copy_bidirectional(&mut stream, &mut child).await?;
    }
}
```

### 4.2. The DHT Integration
We integrate `libp2p-kademlia` or a lightweight Rust DHT implementation.

**In `cell-sdk/src/dht.rs`:**
```rust
pub struct Mycelium {
    // The routing table
    table: RoutingTable,
}

impl Mycelium {
    pub async fn announce(&self, cell_id: [u8; 32]) {
        // Tell the network we have this cell
    }
    
    pub async fn find_providers(&self, cell_id: [u8; 32]) -> Vec<PeerAddr> {
        // Ask the network who has this cell
    }
}
```

## 5. Security Implications

### The Poisoning Attack
*   *Attack:* Malicious peer claims to have `exchange`, sends malware.
*   *Defense:* **Cryptographic Verification.** The build script **must** verify the signature of the received git tag matches the `authority` public key defined in `Cell.toml`.
*   *Outcome:* The malware is downloaded, verification fails, the cache is purged, the peer is blacklisted locally. The build fails safely.

### The Squatting Attack
*   *Attack:* Someone claims they are the "real" exchange.
*   *Defense:* Public Keys are the namespace. You cannot squat `ed25519:7a8f...`.
*   *UX Mitigation:* We can use a local "Petname" system (like `/etc/hosts`) to map friendly names to keys, or rely on a governance cell later.

## 6. Summary

This removes the single point of failure.

*   **Offline?** Works if cached or if a peer is on LAN.
*   **Censorship?** Impossible without shutting down every machine running the cell.
*   **Cost?** $0.00. No hosting bills. Bandwidth is shared by the users.

**Cell does not just run code. Cell keeps code alive.**
```