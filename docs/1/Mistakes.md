I apologize. You are absolutely right. I defaulted to the standard "framework" mindset where the central library (the SDK) dictates how features work. That fundamentally violates the **Cell** philosophy.

Here is why I made that mistake and why your approach is the only one that fits the paradigm:

### Why I did it (The Mistake)
I fell into the trap of thinking "Persistence" was a generic utility that belonged in the toolbox. I was trying to adhere to the **DRY (Don't Repeat Yourself)** principle by putting the `PersistentField` logic in one place (`cell-sdk`) so that multiple different Cells (e.g., a SQL Cell, a Redis Cell) could reuse it.

### Why that was wrong
By putting it in the SDK, I coupled the **substrate** (the SDK) to a **behavior** (persistence).
1.  **Bloat:** The SDK now knows about "dirty checking" and "background saving," which it shouldn't. It should only know about sending bytes.
2.  **Constraint:** If you wanted to write a Storage Cell that worked differently (e.g., push-based instead of poll-based), you'd be fighting the SDK's built-in `PersistentField` implementation.
3.  **Philosophy Violation:** In the Cell model, **the Cell defines the reality**. If a Cell offers storage, *it* alone should define what "storing" looks like on the client side. The SDK is just the carrier signal.

### The Correct Architecture (What you asked for)
The **Storage Cell** must be a self-contained universe. It must export:
1.  The **Server Logic** (the `#[handler]` code running in the container).
2.  The **Client Logic** (the `PersistentField` struct and the `#[store]` macro).

The SDK remains "bare"—it is just the nervous system (transport/discovery). It doesn't know what "Storage" is.

**How it works without the SDK code:**
When `cell-build` compiles the Storage Cell, it shouldn't just extract the proc-macro. It must also extract the **Helper Structs** (like `PersistentField`) defined in the Storage Cell's source code and bundle them into the generated `storage-macros` crate.

The client then imports `storage-macros`. The `PersistentField` struct exists inside that generated crate, tailored specifically by the Storage Cell author, with zero support from the core SDK.