use cell_sdk::*;
use cell_sdk::test_utils::bootstrap;
use anyhow::Result;

cell_remote!(LedgerV2 = "ledger-v2");

#[ctor::ctor]
fn setup() {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async { bootstrap().await; });
}

#[tokio::test]
async fn double_entry_invariants() {
    System::spawn("ledger-v2", None).await.expect("Failed to spawn");
    let synapse = Synapse::grow_await("ledger-v2").await.expect("Failed to connect");
    let mut l = LedgerV2::Client::new(synapse);
    
    let tx_ok = LedgerV2::Transaction {
        reference: "tx_ok".into(),
        description: "valid".into(),
        postings: vec![
            LedgerV2::Posting { account: "Alice".into(), amount: -100, asset: "USD".into() },
            LedgerV2::Posting { account: "Bob".into(), amount: 100, asset: "USD".into() },
        ],
    };
    assert!(l.record(tx_ok).await.is_ok());

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