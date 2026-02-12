use anyhow::Result;
use cell_cli::golgi::pheromones::EndocrineSystem;
use serial_test::serial;
use std::time::Duration;

#[tokio::test]
#[serial] // Serial because UDP multicast ports are shared
async fn test_donor_discovery() -> Result<()> {
    // 1. Start a "Donor" Node Listener
    let mut rx_network = EndocrineSystem::start(
        "observer-node".to_string(),
        "observer".to_string(),
        9000,
        "public_key_observer".to_string(),
        false, // Observer is NOT a donor
        None,  // No IPC socket
    )
    .await?;

    // 2. Start a "Donor" Node Broadcaster
    // It runs in the background automatically upon start
    let _tx_node = EndocrineSystem::start(
        "rich-donor-node".to_string(),
        "worker".to_string(),
        9001,
        "public_key_donor".to_string(),
        true, // <--- THIS NODE IS A DONOR
        None, // No IPC socket
    )
    .await?;

    // 3. Start a "Leech" Node Broadcaster
    let _leech_node = EndocrineSystem::start(
        "poor-leech-node".to_string(),
        "client".to_string(),
        9002,
        "public_key_leech".to_string(),
        false, // <--- THIS NODE IS NOT A DONOR
        None,  // No IPC socket
    )
    .await?;

    // 4. Listen for advertisements
    let mut found_donor = false;
    let mut found_leech = false;

    // Scan for 2 seconds max
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        if let Ok(pheromone) =
            tokio::time::timeout(Duration::from_millis(500), rx_network.recv()).await
        {
            if let Some(p) = pheromone {
                println!("Observed Pheromone: {:?}", p);

                if p.cell_name == "rich-donor-node" {
                    assert_eq!(
                        p.is_donor, true,
                        "Donor node failed to advertise donor status"
                    );
                    found_donor = true;
                }

                if p.cell_name == "poor-leech-node" {
                    assert_eq!(
                        p.is_donor, false,
                        "Leech node falsely advertised donor status"
                    );
                    found_leech = true;
                }
            }
        }

        if found_donor && found_leech {
            break;
        }
    }

    assert!(found_donor, "Failed to discover donor node via multicast");
    assert!(found_leech, "Failed to discover leech node via multicast");

    Ok(())
}
