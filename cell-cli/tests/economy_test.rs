use anyhow::Result;
use cell_cli::mitochondria::Mitochondria;

#[test]
fn test_atp_calculation() {
    let temp = tempfile::tempdir().unwrap();
    let mito = Mitochondria::load_or_init(temp.path()).unwrap();

    // 100ms = 1 ATP
    assert_eq!(mito.calculate_cost(100), 1);
    // 200ms = 2 ATP
    assert_eq!(mito.calculate_cost(200), 2);
    // 50ms = 1 ATP (Minimum charge)
    assert_eq!(mito.calculate_cost(50), 1);
    // 101ms = 2 ATP (Ceiling)
    assert_eq!(mito.calculate_cost(101), 2);
}

#[test]
fn test_ledger_transaction_flow() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let mito = Mitochondria::load_or_init(temp.path())?;

    // Initial state
    assert_eq!(mito.get_balance(), 0);

    // 1. Earn ATP (I worked for someone)
    // Job took 500ms -> Should earn 5 ATP
    let earned = mito.synthesize_atp("peer-a", "job-1", 500)?;
    assert_eq!(earned, 5);
    assert_eq!(mito.get_balance(), 5);

    // 2. Spend ATP (Someone worked for me)
    // Job took 200ms -> Should burn 2 ATP
    let spent = mito.burn_atp("peer-b", "job-2", 200)?;
    assert_eq!(spent, 2);
    assert_eq!(mito.get_balance(), 3); // 5 - 2 = 3

    Ok(())
}

#[test]
fn test_ledger_persistence() -> Result<()> {
    let temp = tempfile::tempdir()?;

    // 1. Open Ledger, Earn money, Drop it.
    {
        let mito = Mitochondria::load_or_init(temp.path())?;
        mito.synthesize_atp("peer-x", "job-z", 1000)?; // +10 ATP
        assert_eq!(mito.get_balance(), 10);
        // mito goes out of scope here, should have saved to disk
    }

    // 2. Re-open Ledger from same path
    let mito_reloaded = Mitochondria::load_or_init(temp.path())?;

    // 3. Verify Balance persisted
    assert_eq!(mito_reloaded.get_balance(), 10);

    Ok(())
}
