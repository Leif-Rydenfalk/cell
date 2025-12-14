Trivial in terms of *code*; the heavy part is re-using the **infrastructure you already paid for** (consensus, sandboxing, hot-swap, telemetry) instead of re-building it inside a new micro-service framework.

Concrete delta for a **loan-cell** sitting next to your existing marketplace-cell:

--------------------------------------------------------------------
1. Schema (5 min)
--------------------------------------------------------------------
```rust
#[protein(class = "LoanV1")]
pub enum LoanMsg {
    Request { customer_id: u64, amount: u64, term_days: u32 },
    Approve  { loan_id: u64, apr_bp: u16 },
    Repay    { loan_id: u64, amount: u64 },
    Balance  { loan_id: u64 },
}
```

--------------------------------------------------------------------
2. Business logic (1–2 d)
--------------------------------------------------------------------
- Pure Rust, no I/O: read-only queries hit an in-memory KV (backed by `cell-consensus` log).  
- All mutations go through the same Raft leader that already sequences marketplace trades; you literally `raft.propose(cmd).await` and you’re done—**deterministic replay, audit trail, byzantine safety for free**.  
- Risk engine is a deterministic function (scorecard, DTI, etc.) so you can unit-test it with captured event streams.

--------------------------------------------------------------------
3. Persistence / audit (0 extra work)
--------------------------------------------------------------------
- WAL is already hashed and replicated; regulators get a **single linear log** containing *both* trades and loan events—no ETL glue required.

--------------------------------------------------------------------
4. Integration touch-points (half day each)
--------------------------------------------------------------------
- **Marketplace** → **Loan**: fire `LoanMsg::Request` when checkout basket contains “Pay-Later” option.  
- **Loan** → **Marketplace**: fire `MarketMsg::PlaceOrder` when loan is approved (so inventory is reserved atomically).  
- **Customer portal**: same `Synapse::grow("loan")` call the trader cell used; no new REST layer.

--------------------------------------------------------------------
5. Security / compliance (already solved)
--------------------------------------------------------------------
- Capsid gives you **read-only root FS** and **no network** by default; loan cell literally cannot exfiltrate data even if compromised.  
- Each cell runs under its own UID/GID with cgroup memory limit; you can **prove** to auditors that the loan engine can never allocate more than, say, 512 MB.

--------------------------------------------------------------------
6. Roll-out / rollback
--------------------------------------------------------------------
```bash
cell spawn loan-v1.0.0
cell swap loan            # atomic, < 16 ms
# …if trouble…
cell swap loan-v0.9.0     # instant rollback, zero data loss
```

--------------------------------------------------------------------
Bottom line
--------------------------------------------------------------------
- **New business code**: ~500 lines Rust.  
- **New infrastructure**: **zero**—you already built the distributed operating system.  
- **Time-to-market**: **< 1 week** for an MVP that is **provably deterministic, auditable, sandboxed, and hot-swappable**.

Compare that with a typical bank micro-service:  
- new K8s manifests, new DB schema, new REST contract, new Kafka topic, new IAM roles, new CVE scanning pipeline, new rollback playbook…  
You just **added a cell** instead of a whole vertical.