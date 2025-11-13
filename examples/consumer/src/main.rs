use cell_sdk::*;
use anyhow::Result;

fn main() -> Result<()> {
    println!("🧪 Testing calculator service with compile-time types\n");
    
    // Compile-time checked call - types generated automatically!
    let response = call_as!(calculator, CalcRequest {
        operation: "add".to_string(),
        a: 10.0,
        b: 5.0,
    })?;
    
    println!("✓ 10 + 5 = {}", response.result);
    
    let response = call_as!(calculator, CalcRequest {
        operation: "multiply".to_string(),
        a: 6.0,
        b: 7.0,
    })?;
    
    println!("✓ 6 × 7 = {}", response.result);
    
    Ok(())
}
