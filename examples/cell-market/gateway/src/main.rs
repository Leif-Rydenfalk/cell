use axum::{routing::post, Router, Json};
use cell_sdk::cell_remote;
use serde::Deserialize;

// --- SYMBIOSIS ---
cell_remote!(Ledger = "ledger");
cell_remote!(Engine = "engine");

#[derive(Deserialize)]
struct DepositReq { user: u64, amount: u64 }

async fn deposit(Json(req): Json<DepositReq>) -> String {
    let mut client = match Ledger::connect().await {
        Ok(c) => c,
        Err(_) => return "Ledger Down".into(),
    };

    match client.deposit(req.user, Ledger::Asset::USD, req.amount).await {
        Ok(Ok(bal)) => format!("Deposited. New Balance: {}", bal),
        Ok(Err(e)) => format!("Ledger Error: {}", e),
        Err(e) => format!("Network Error: {}", e),
    }
}

#[derive(Deserialize)]
struct OrderReq { user: u64, side: String, amount: u64, price: u64 }

async fn trade(Json(req): Json<OrderReq>) -> String {
    let mut client = match Engine::connect().await {
        Ok(c) => c,
        Err(_) => return "Engine Down".into(),
    };

    let side = if req.side == "buy" { Engine::Side::Buy } else { Engine::Side::Sell };

    match client.place_order(req.user, "BTC-USD".into(), side, req.price, req.amount).await {
        Ok(Ok(id)) => format!("Order Placed. ID: {}", id),
        Ok(Err(e)) => format!("Engine Rejected: {}", e),
        Err(e) => format!("Network Error: {}", e),
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