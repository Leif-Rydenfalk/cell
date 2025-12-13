use cell_sdk::crdt::{GCounter, LwwRegister};

#[test]
fn test_gcounter_convergence() {
    // Node A
    let mut a = GCounter::new(1);
    a.inc();
    a.inc();
    assert_eq!(a.value(), 2);

    // Node B
    let mut b = GCounter::new(2);
    b.inc();
    assert_eq!(b.value(), 1);

    // Network Partition: A updates
    a.inc(); // A=3

    // Network Heal: Sync B -> A
    a.merge(&b);
    // A has 3 from self, 1 from B. Total 4.
    assert_eq!(a.value(), 4);

    // Sync A -> B
    b.merge(&a);
    // B has 1 from self, 3 from A. Total 4.
    assert_eq!(b.value(), 4);
    
    assert_eq!(a.value(), b.value());
}

#[test]
fn test_lww_register() {
    let mut reg_a = LwwRegister::new("Initial", 100);
    let mut reg_b = LwwRegister::new("Initial", 100);

    // Update A with later timestamp
    reg_a.set("Update A", 110);
    
    // Update B with even later timestamp
    reg_b.set("Update B", 120);

    // Merge B into A
    reg_a.merge(&reg_b);
    assert_eq!(*reg_a.get(), "Update B");

    // Merge A (now B) into B - should be no change
    reg_b.merge(&reg_a);
    assert_eq!(*reg_b.get(), "Update B");
}