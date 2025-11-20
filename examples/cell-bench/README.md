# Just copy and past this whole file and paste into terminal and run.

# --- 1. Clean up previous artifacts and runtime state ---
cargo clean
rm -rf worker/target aggregator/target coordinator/target
rm -rf worker/run aggregator/run coordinator/run

# --- 2. Stop any lingering processes ---
cell stop worker 
cell stop aggregator 
cell stop coordinator  

# --- 3. Build and Start Services ---
cell run worker

cell run aggregator

cell run coordinator

# --- 4. Execute Benchmark ---
# Scenario 1: Maximum Bandwidth (GB/s)
# This sends large payloads (e.g., 50KB) using 8 threads to saturate the data transfer capacity:
cell use coordinator run '{"test_type": "bandwidth", "iterations": 20000, "payload_size": 51200, "worker_count": 8}'

# Scenario 2: Maximum RPC Rate (Operations/sec)
# This sends small payloads (1KB) using 16 threads to stress the transaction processing overhead:
cell use coordinator run '{"test_type": "bandwidth", "iterations": 100000, "payload_size": 1024, "worker_count": 16}'

# --- 5. Check Logs ---
tail -n 20 coordinator/run/nucleus.log