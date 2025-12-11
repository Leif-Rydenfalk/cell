// cells/firewall/src/main.rs
// SPDX-License-Identifier: MIT
// Zero-Trust Network Policy Enforcement

use cell_sdk::*;
use anyhow::Result;
use std::sync::Arc;
use std::net::IpAddr;
use tokio::sync::RwLock;
use ipnet::IpNet;
use std::time::{Instant, Duration};

// === PROTOCOL ===

#[protein]
pub struct FirewallRule {
    pub id: String,
    pub priority: u32,
    pub action: RuleAction,
    pub source_cidr: String,
    pub destination_cell: String,
    pub rate_limit_rps: Option<u32>,
}

#[protein]
pub enum RuleAction {
    Allow,
    Deny,
    LogOnly,
}

#[protein]
pub struct CheckRequest {
    pub source_ip: String,
    pub target_cell: String,
}

#[protein]
pub struct CheckDecision {
    pub allowed: bool,
    pub reason: String,
}

// === SERVICE ===

struct RateLimiter {
    tokens: f64,
    last_update: Instant,
    capacity: f64,
    refill_rate: f64,
}

impl RateLimiter {
    fn new(rps: u32) -> Self {
        Self {
            tokens: rps as f64,
            last_update: Instant::now(),
            capacity: rps as f64,
            refill_rate: rps as f64,
        }
    }

    fn check(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_update = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct FirewallState {
    rules: Vec<FirewallRule>, // Sorted by priority
    rate_limiters: std::collections::HashMap<String, RateLimiter>, // Key: "IP:RuleID"
}

#[service]
#[derive(Clone)]
struct FirewallService {
    state: Arc<RwLock<FirewallState>>,
}

impl FirewallService {
    fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(FirewallState {
                rules: Vec::new(),
                rate_limiters: std::collections::HashMap::new(),
            })),
        }
    }

    fn matches(rule: &FirewallRule, ip: IpAddr, target: &str) -> bool {
        if rule.destination_cell != "*" && rule.destination_cell != target {
            return false;
        }
        
        if rule.source_cidr == "*" {
            return true;
        }

        if let Ok(net) = rule.source_cidr.parse::<IpNet>() {
            net.contains(&ip)
        } else {
            false
        }
    }
}

#[handler]
impl FirewallService {
    async fn add_rule(&self, rule: FirewallRule) -> Result<bool> {
        let mut state = self.state.write().await;
        state.rules.push(rule);
        state.rules.sort_by(|a, b| a.priority.cmp(&b.priority)); // Ascending priority (0 is highest)
        Ok(true)
    }

    async fn check(&self, req: CheckRequest) -> Result<CheckDecision> {
        let mut state = self.state.write().await;
        let ip: IpAddr = req.source_ip.parse().unwrap_or_else(|_| "0.0.0.0".parse().unwrap());

        for rule in &state.rules {
            if Self::matches(rule, ip, &req.target_cell) {
                // Rate Limit Check
                if let Some(rps) = rule.rate_limit_rps {
                    let key = format!("{}:{}", req.source_ip, rule.id);
                    let limiter = state.rate_limiters.entry(key).or_insert_with(|| RateLimiter::new(rps));
                    
                    if !limiter.check() {
                        return Ok(CheckDecision {
                            allowed: false,
                            reason: "Rate Limit Exceeded".to_string(),
                        });
                    }
                }

                return match rule.action {
                    RuleAction::Allow => Ok(CheckDecision { allowed: true, reason: format!("Matched Rule {}", rule.id) }),
                    RuleAction::Deny => Ok(CheckDecision { allowed: false, reason: format!("Denied by Rule {}", rule.id) }),
                    RuleAction::LogOnly => {
                        tracing::info!("[Firewall] LOG: {} -> {} matched rule {}", req.source_ip, req.target_cell, rule.id);
                        continue; // Continue processing lower priority rules
                    }
                };
            }
        }

        // Default Deny
        Ok(CheckDecision {
            allowed: false,
            reason: "Default Deny".to_string(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    tracing::info!("[Firewall] Network Policy Engine Active");
    
    let service = FirewallService::new();
    
    // Add default allow-local rule
    service.add_rule(FirewallRule {
        id: "default-local".into(),
        priority: 100,
        action: RuleAction::Allow,
        source_cidr: "127.0.0.0/8".into(),
        destination_cell: "*".into(),
        rate_limit_rps: None,
    }).await?;

    service.serve("firewall").await
}