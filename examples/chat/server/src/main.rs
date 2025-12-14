// chat/server/src/main.rs
use cell_sdk::*;
use std::sync::Arc;
use tokio::sync::RwLock;

#[protein]
pub struct Message {
    pub user: String,
    pub text: String,
    pub timestamp: u64,
}

#[protein]
pub struct SendRequest {
    pub user: String,
    pub text: String,
}

#[protein]
pub struct HistoryRequest {
    pub limit: u32,
}

// In-memory message buffer (last 1000 messages)
struct ChatState {
    messages: Vec<Message>,
}

#[service]
#[derive(Clone)]
struct ChatService {
    state: Arc<RwLock<ChatState>>,
}

#[handler]
impl ChatService {
    // Broadcast to all connected clients
    async fn send(&self, req: SendRequest) -> Result<u64> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        let msg = Message {
            user: req.user.clone(),
            text: req.text.clone(),
            timestamp,
        };
        
        let mut state = self.state.write().await;
        state.messages.push(msg.clone());
        
        // Keep only last 1000 messages
        if state.messages.len() > 1000 {
            state.messages.drain(0..state.messages.len() - 1000);
        }
        
        println!("[{}] {}: {}", timestamp, req.user, req.text);
        
        Ok(timestamp)
    }
    
    // Get recent message history
    async fn history(&self, req: HistoryRequest) -> Result<Vec<Message>> {
        let state = self.state.read().await;
        let start = state.messages.len().saturating_sub(req.limit as usize);
        Ok(state.messages[start..].to_vec())
    }
    
    // Stream live messages (long-polling)
    async fn stream(&self, since: u64) -> Result<Vec<Message>> {
        let state = self.state.read().await;
        Ok(state.messages.iter()
            .filter(|m| m.timestamp > since)
            .cloned()
            .collect())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .init();
    
    println!("💬 Chat Server Online");
    
    let service = ChatService {
        state: Arc::new(RwLock::new(ChatState {
            messages: Vec::new(),
        })),
    };
    
    service.serve("chat").await
}