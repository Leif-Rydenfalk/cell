use axum::{routing::post, Router, Json};
use cell_sdk::cell_remote;
use serde::Deserialize;

// --- SYMBIOSIS ---
cell_remote!(ledger = "ledger");
cell_remote!(engine = "engine");

#[derive(Deserialize)]
struct DepositReq { user: u64, amount: u64 }

async fn deposit(Json(req): Json<DepositReq>) -> String {
    let mut client = match ledger::connect().await {
        Ok(c) => c,
        Err(_) => return "Ledger Down".into(),
    };

    // Client returns Result<u64, CellError> (unwrapped by macro) or similar?
    // Let's re-verify the macro logic in `cell-macros/src/lib.rs`:
    // It emits: `-> ::std::result::Result<#wret, ::cell_sdk::CellError>`
    // where #wret is the return type of the handler.
    // In `ledger/src/main.rs`, handler returns `Result<u64>`.
    // And `sanitize_return_type` extracts `T` from `Result<T>`.
    // So the client returns `Result<u64, CellError>`.
    // The previous error message `expected u64, found Result<_, _>` implies the outer result was matched, 
    // but the inner value was still a Result?
    // Wait, if sanitize_return_type works, it returns `Result<u64, CellError>`.
    // So `Ok(bal)` matches `Ok(u64)`.
    
    // Ah, the previous error was:
    // `match client.deposit(...)` -> type `Result<u64, CellError>`
    // `Ok(Ok(bal))` -> expected `u64`, found `Result`.
    // This implies `client.deposit` returned `Result<Result<u64, ...>, ...>` OR `Result<u64, ...>`.
    // If macro returns `Result<u64, CellError>`, then `Ok(bal)` is correct.
    // The previous error said: `expected u64, found Result`.
    // This means the pattern `Ok(Ok(bal))` was matching against `Result<Result<...>>`.
    // So `client.deposit` returned a nested result.
    
    // BUT, I just fixed `sanitize_return_type` in `cell-macros` to strip the result!
    // So now it should return `Result<u64, CellError>`.
    // Therefore `Ok(bal)` is the correct pattern. `Ok(Ok(bal))` would be wrong.
    // Let's assume the macro fix worked and use single unwrapping.

    match client.deposit(req.user, ledger::Asset::USD, req.amount).await {
        Ok(bal) => format!("Deposited. New Balance: {}", bal),
        Err(e) => format!("Ledger Error: {}", e),
    }
}

#[derive(Deserialize)]
struct OrderReq { user: u64, side: String, amount: u64, price: u64 }

async fn trade(Json(req): Json<OrderReq>) -> String {
    let mut client = match engine::connect().await {
        Ok(c) => c,
        Err(_) => return "Engine Down".into(),
    };

    let side = if req.side == "buy" { engine::Side::Buy } else { engine::Side::Sell };

    let rpc_result = client.place_order(req.user, "BTC-USD".into(), side, req.price, req.amount).await;

    match rpc_result {
        Ok(id) => format!("Order Placed. ID: {}", id),
        Err(e) => format!("Engine Error: {}", e),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let app = Router::new()
        .route("/deposit", post(deposit))
        .route("/trade", post(trade));
    println!("Gateway: http://0.0.0.0:8080");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}