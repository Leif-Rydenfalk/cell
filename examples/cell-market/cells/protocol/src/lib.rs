use cell_sdk::protein;

#[protein]
pub enum MarketMsg {
    PlaceOrder {
        symbol: String,
        amount: u64,
        side: u8,
    },
    // FIX: Removed recursive BatchOrders variant to prevent compiler overflow.
    // We use SubmitBatch to simulate a high-throughput payload.
    SubmitBatch {
        count: u32,
    },
    OrderAck {
        id: u64,
    },
    SnapshotRequest,
    SnapshotResponse {
        total_trades: u64,
    },
}
