// hello/src/main.rs
use cell_sdk::*;

#[protein]
pub struct Ping {
    pub msg: String,
}

#[service]
#[derive(Clone)]
struct HelloService;

#[handler]
impl HelloService {
    async fn ping(&self, req: Ping) -> Result<String> {
        println!("📨 Received: {}", req.msg);
        Ok(format!("Hello from {}! You said: {}", 
            std::env::var("HOSTNAME").unwrap_or("unknown".into()),
            req.msg
        ))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    
    println!("🔬 Cell 'hello' starting...");
    
    let service = HelloService;
    service.serve("hello").await
}