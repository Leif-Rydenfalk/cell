use cell_sdk::protein;

#[protein]
pub enum MarketMsg {
    PlaceOrder {
        symbol: String,
        amount: u64,
        side: u8,
    },
    OrderAck {
        id: u64,
    },
    SnapshotRequest,
    SnapshotResponse {
        total_trades: u64,
    },
}
