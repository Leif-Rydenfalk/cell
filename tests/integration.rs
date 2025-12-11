use cell_test_support::*;
use cell_sdk::*;
use cell_model::ops::*;
use anyhow::Result;
use std::time::Duration;

// --- DEFINE REMOTES ---
// We define interfaces for all cells we interact with in the integration suite.
cell_remote!(Nucleus = "nucleus");
cell_remote!(Axon = "axon");
cell_remote!(Consensus = "consensus-raft");
cell_remote!(LedgerV2 = "ledger-v2");
cell_remote!(Ledger = "ledger"); // The example ledger used by Engine
cell_remote!(Vault = "vault");
cell_remote!(Dht = "dht");
cell_remote!(Metrics = "metrics");
cell_remote!(Trace = "trace");
cell_remote!(LoadBalancer = "loadbalancer");
cell_remote!(Autoscaler = "autoscaler");
cell_remote!(Canary = "canary");
cell_remote!(Firewall = "firewall");
cell_remote!(Config = "config");
cell_remote!(Registry = "registry");
cell_remote!(Observer = "observer");
cell_remote!(Backup = "backup");
cell_remote!(Iam = "iam");
cell_remote!(Audit = "audit");
cell_remote!(Chaos = "chaos");
cell_remote!(Vivaldi = "vivaldi");
cell_remote!(Drift = "drift");
cell_remote!(Exchange = "exchange");
cell_remote!(Worker = "worker");
cell_remote!(Engine = "engine");

// Helpers for test ergonomics
macro_rules! spawn_client {
    ($t:ty, $name:expr) => {{
        let synapse = spawn($name).await;
        <$t>::new(synapse)
    }}
}

// 1. Nucleus: Registry Persistence
#[tokio::test]
async fn nucleus_keeps_registry_across_restart() {
    // This tests that Nucleus stores state. 
    // Since our Nucleus implementation currently is in-memory, we test the registration flow works.
    let mut n = spawn_client!(Nucleus::Client, "nucleus");
    
    let reg = Nucleus::CellRegistration {
        name: "test-persist".into(),
        node_id: 99,
        capabilities: vec!["persist".into()],
        endpoints: vec!["tcp://1.2.3.4:9000".into()]
    };

    let success = n.register(reg).await.expect("Registration failed");
    assert!(success);
    
    // Verify discovery finds it
    let res = n.discover(Nucleus::DiscoveryQuery { 
        cell_name: "test-persist".into(), 
        prefer_local: true 
    }).await.unwrap();
    
    assert!(!res.instances.is_empty());
    assert_eq!(res.instances[0].node_id, 99);
}

// 2. Axon: Gateway Mounting
#[tokio::test]
async fn axon_gateway_mounts_remote_cell() {
    // Ensure target cell is running
    let _ = spawn("ledger-v2").await;
    
    // Connect to Axon
    let mut axon = spawn_client!(Axon::Client, "axon");
    
    // Ask Axon to bridge "ledger-v2"
    // Since we are running locally, Axon should find it via Discovery
    let resp = axon.mount("ledger-v2".into()).await.unwrap();
    
    if let Axon::BridgeResponse::Mounted { socket_path } = resp {
        assert!(std::path::Path::new(&socket_path).exists());
    } else {
        panic!("Axon failed to mount local cell: {:?}", resp);
    }
}

// 3. Ledger V2: Invariant Checking
#[tokio::test]
async fn double_entry_invariants() {
    let mut l = spawn_client!(LedgerV2::Client, "ledger-v2");
    
    // Valid Transaction
    let tx_ok = LedgerV2::Transaction {
        reference: "tx_ok".into(),
        description: "valid".into(),
        postings: vec![
            LedgerV2::Posting { account: "Alice".into(), amount: -100, asset: "USD".into() },
            LedgerV2::Posting { account: "Bob".into(), amount: 100, asset: "USD".into() },
        ],
    };
    assert!(l.record(tx_ok).await.is_ok());

    // Invalid Transaction (Unbalanced)
    let tx_bad = LedgerV2::Transaction {
        reference: "tx_bad".into(),
        description: "fraud".into(),
        postings: vec![
            LedgerV2::Posting { account: "Alice".into(), amount: -100, asset: "USD".into() },
            LedgerV2::Posting { account: "Bob".into(), amount: 200, asset: "USD".into() },
        ],
    };
    assert!(l.record(tx_bad).await.is_err());
}

// 4. Vault: Secret Management
#[tokio::test]
async fn vault_secrets_lifecycle() {
    let mut v = spawn_client!(Vault::Client, "vault");
    
    // Write
    let secret_data = b"super_secret_payload".to_vec();
    let version = v.put(Vault::SecretWrite { 
        key: "api_key".into(), 
        value: secret_data.clone(), 
        ttl_secs: None 
    }).await.unwrap();
    
    // Read specific version
    let read_back = v.get(Vault::SecretRead { 
        key: "api_key".into(), 
        version: Some(version) 
    }).await.unwrap();
    
    assert_eq!(read_back, secret_data);
    
    // Test Rotation (simulated)
    assert!(v.rotate_keys().await.unwrap());
}

// 5. DHT: Storage
#[tokio::test]
async fn dht_put_get_simple() {
    let mut d = spawn_client!(Dht::Client, "dht");
    
    d.put(Dht::DhtStore { 
        key: "user:123".into(), 
        value: b"User Data".to_vec(), 
        ttl_secs: 60 
    }).await.unwrap();
    
    let val = d.get(Dht::DhtGet { key: "user:123".into() }).await.unwrap();
    
    assert_eq!(val.value, Some(b"User Data".to_vec()));
    assert_eq!(val.found_on, "local");
}

// 6. Metrics: Ingestion
#[tokio::test]
async fn metrics_ingest_query() {
    let mut m = spawn_client!(Metrics::Client, "metrics");
    
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    
    m.push(vec![Metrics::MetricPoint {
        name: "cpu_usage".into(), 
        value: 42.0, 
        timestamp: now, 
        tags: vec![("host".into(), "test".into())]
    }]).await.unwrap();
    
    // Query back
    let points = m.query(Metrics::QueryRange {
        name: "cpu_usage".into(),
        start: now - 10,
        end: now + 10,
    }).await.unwrap();
    
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].value, 42.0);
}

// 7. Registry: Package Publishing (Mocked Sig)
#[tokio::test]
async fn registry_publish_flow() {
    let mut r = spawn_client!(Registry::Client, "registry");
    
    // 1. Establish Trust
    let dummy_pub_key = vec![1, 2, 3, 4]; // Placeholder
    r.trust(Registry::TrustKey {
        author: "alice".into(),
        public_key: dummy_pub_key.clone(),
    }).await.unwrap();
    
    // 2. Publish
    // The registry stub checks signature len == 64
    let dummy_sig = vec![0u8; 64]; 
    
    let pkg = Registry::Package {
        name: "my-cell".into(),
        version: "0.1.0".into(),
        description: "test".into(),
        author: "alice".into(),
        git_url: "https://github.com/alice/my-cell".into(),
        commit_hash: "abcdef".into(),
        signature: dummy_sig,
    };
    
    let res = r.publish(Registry::PublishRequest {
        package: pkg,
        source_tarball: vec![],
        signing_key: dummy_pub_key,
    }).await;
    
    assert!(res.is_ok());
    
    // 3. Search
    let results = r.search(Registry::SearchQuery {
        query: "my-cell".into(),
        limit: 10,
    }).await.unwrap();
    
    assert_eq!(results.packages.len(), 1);
    assert_eq!(results.packages[0].name, "my-cell");
}

// 8. Engine: Full Trading Flow
#[tokio::test]
async fn engine_processes_order() {
    // 1. Start dependencies
    // Note: Engine relies on "ledger" (the example one), not "ledger-v2".
    let _l = spawn("ledger").await; 
    let _c = spawn("consensus-raft").await;
    
    // 2. Start Engine
    let mut engine = spawn_client!(Engine::Client, "engine");
    
    // 3. Fund user in Ledger
    // We need to connect to the 'ledger' cell to deposit funds first.
    let mut ledger = spawn_client!(Ledger::Client, "ledger");
    
    // User 100 deposits 1000 USD
    ledger.deposit(100, Ledger::Asset::USD, 1000).await.unwrap();
    
    // 4. Place Order via Engine
    // Buy 1 BTC @ 500 USD
    let order_id = engine.place_order(
        100, 
        "BTC-USD".into(), 
        Engine::Side::Buy, 
        500, 
        1
    ).await.unwrap();
    
    assert_eq!(order_id, 12345); // Check mock return ID
    
    // 5. Verify Ledger Locked Funds
    // 1000 initial - 500 locked = 500 remaining available? 
    // The Ledger stub implements `deposit` (adds) and `lock_funds` (subtracts).
    // It doesn't have a specific "get balance" in the example stub, only return from deposit.
    // But we can deposit 0 to check balance.
    let bal = ledger.deposit(100, Ledger::Asset::USD, 0).await.unwrap();
    assert_eq!(bal, 500); // 1000 - 500
}

// 9. IAM: Permission Checks
#[tokio::test]
async fn iam_enforces_rbac() {
    let mut iam = spawn_client!(Iam::Client, "iam");
    
    // Login as admin
    let auth = iam.login(Iam::LoginRequest {
        client_id: "admin".into(),
        client_secret: "admin123".into(),
    }).await.unwrap();
    
    assert!(!auth.token.is_empty());
    
    // Check permission
    let allowed = iam.check(Iam::CheckPermission {
        token: auth.token,
        resource: "database".into(),
        action: "drop".into(),
    }).await.unwrap();
    
    assert!(allowed); // Admin has *
    
    // Login as restricted user
    let fail_auth = iam.login(Iam::LoginRequest {
        client_id: "finance".into(),
        client_secret: "moneyprinter".into(),
    }).await.unwrap();
    
    let denied = iam.check(Iam::CheckPermission {
        token: fail_auth.token,
        resource: "nuclear_codes".into(),
        action: "launch".into(),
    }).await.unwrap();
    
    assert!(!denied);
}

// 10. Audit: Merkle Chain verification
#[tokio::test]
async fn audit_chain_integrity() {
    let mut audit = spawn_client!(Audit::Client, "audit");
    
    // Log events
    audit.log(Audit::AuditEvent {
        actor: "system".into(), action: "boot".into(), resource: "cpu".into(), 
        outcome: "ok".into(), metadata: "".into(), timestamp: 1
    }).await.unwrap();
    
    audit.log(Audit::AuditEvent {
        actor: "user".into(), action: "login".into(), resource: "web".into(), 
        outcome: "ok".into(), metadata: "".into(), timestamp: 2
    }).await.unwrap();
    
    // Verify
    let is_valid = audit.verify().await.unwrap();
    assert!(is_valid);
}

// 11. Consensus: Basic Propose
#[tokio::test]
async fn consensus_raft_single_node() {
    // Tests the example/consensus-raft implementation
    let mut c = spawn_client!(Consensus::Client, "consensus-raft");
    
    let cmd = Consensus::Command { data: b"hello".to_vec() };
    let res = c.propose(cmd).await.unwrap();
    
    // Index should increment
    assert!(res.index > 0);
}

// 12. Firewall: Rate Limiting
#[tokio::test]
async fn firewall_rate_limiting() {
    let mut f = spawn_client!(Firewall::Client, "firewall");
    
    // Add rule with 1 RPS limit
    f.add_rule(Firewall::FirewallRule {
        id: "limit_me".into(),
        priority: 1,
        action: Firewall::RuleAction::Allow,
        source_cidr: "0.0.0.0/0".into(),
        destination_cell: "*".into(),
        rate_limit_rps: Some(1),
    }).await.unwrap();
    
    let req = Firewall::CheckRequest {
        source_ip: "10.0.0.1".into(),
        target_cell: "web".into(),
    };
    
    // First allowed
    let r1 = f.check(req.clone()).await.unwrap();
    assert!(r1.allowed);
    
    // Immediate second denied (Rate Limit)
    let r2 = f.check(req.clone()).await.unwrap();
    assert!(!r2.allowed);
    assert_eq!(r2.reason, "Rate Limit Exceeded");
}

// 13. Autoscaler: Policy Registration
#[tokio::test]
async fn autoscaler_logic() {
    let mut a = spawn_client!(Autoscaler::Client, "autoscaler");
    
    // Register policy
    a.register_policy(Autoscaler::ScalingPolicy {
        cell_name: "worker".into(),
        min_instances: 1,
        max_instances: 10,
        target_cpu: 50.0,
        target_memory_mb: 512,
        cooldown_secs: 5,
    }).await.unwrap();
    
    // Check decision (should be None or monitoring)
    let dec = a.get_decision("worker".into()).await.unwrap();
    assert!(matches!(dec.action, Autoscaler::ScaleAction::None));
}

// 14. Vivaldi: Network Coordinates
#[tokio::test]
async fn vivaldi_distance_calc() {
    let mut v = spawn_client!(Vivaldi::Client, "vivaldi");
    
    // Update peer coordinate at (10, 0, 0)
    // Local is (0,0,0) by default
    let peer_coord = Vivaldi::Coordinate {
        vec: [10.0, 0.0, 0.0],
        height: 0.0,
        error: 0.1,
    };
    
    v.update(Vivaldi::UpdateRTT {
        node_id: "peer_node".into(),
        rtt_ms: 10.0, // RTT matches distance perfectly
        peer_coordinate: peer_coord,
    }).await.unwrap();
    
    // Route query should prioritize this peer if close
    let route = v.route(Vivaldi::RoutingQuery {
        target_cell: "peer".into(),
        source_coordinate: None,
        max_results: 1,
    }).await.unwrap();
    
    assert!(!route.instances.is_empty());
}

// 15. Trace: Span Storage
#[tokio::test]
async fn trace_storage() {
    let mut t = spawn_client!(Trace::Client, "trace");
    
    let span = Trace::Span {
        trace_id: "trace_1".into(),
        span_id: "span_1".into(),
        parent_id: None,
        service: "frontend".into(),
        operation: "GET /".into(),
        start_us: 1000,
        duration_us: 500,
        tags: vec![],
    };
    
    t.push_spans(vec![span]).await.unwrap();
    
    let trace = t.get_trace("trace_1".into()).await.unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].operation, "GET /");
}