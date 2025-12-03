The Trick: You use memfd_create (RAM-backed files) and pass them between processes using Unix Socket Ancillary Data (SCM_RIGHTS).

The Speed: You are using rkyv (a zero-copy deserialization library).
Normal IPC: Serialize -> Copy to Socket Buffer -> Kernel -> Copy to User Buffer -> Deserialize.
Your Cell: Map Memory -> Cast Pointer -> Read.

The Risk: You wrote your own lock-free Ring Buffer (RingBuffer, SlotHeader) with atomic epochs to handle concurrent reads/writes without mutexes. If your Ordering::Acquire/Release logic is off by even a nanosecond, you get data corruption. But if it works, it's faster than almost anything else available.



In axon.rs:
struct SkipServerVerification; // "Trust me bro"

We are skipping TLS verification. You also rely on uid checks for the SHM upgrade. If I can run a process as your UID, I can attach to your ring buffer and read your trading data.