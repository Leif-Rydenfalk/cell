//! Inventory Cell - SQLite Persistence
//!
//! This cell provides:
//! - Persistent item storage with ACID transactions
//! - Player inventories, cargo holds, warehouse storage
//! - Atomic transfer operations
//! - Queryable item database

use anyhow::Result;
use cell_sdk::*;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

// ========= PROTEINS (PUBLIC API) =========

#[protein]
pub struct Item {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: ItemCategory,
    pub rarity: Rarity,
    pub mass: f32,
    pub volume: f32,
    pub base_value: u64,
    pub max_stack: u32,
    // Changed to Vec for reliable rkyv serialization
    pub properties: Vec<(String, String)>,
}

#[protein]
pub enum ItemCategory {
    Ore,
    Refined,
    Component,
    Tool,
    Weapon,
    Thruster,
    Reactor,
    Cargo,
    Drill,
    Scanner,
    Shield,
    Utility,
    Consumable,
    Quest,
}

#[protein]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
    Mythic,
}

#[protein]
pub struct InventoryEntry {
    pub player_id: u64,
    pub item_id: String,
    pub quantity: u64,
    pub durability: f32,
    pub custom_data: Option<String>,
    pub slot: Option<u32>,
}

#[protein]
pub struct CreateInventory {
    pub player_id: u64,
    pub capacity: u32,
}

#[protein]
pub struct DepositItem {
    pub player_id: u64,
    pub item_id: String,
    pub quantity: u64,
    pub durability: Option<f32>,
    pub custom_data: Option<String>,
}

#[protein]
pub struct WithdrawItem {
    pub player_id: u64,
    pub item_id: String,
    pub quantity: u64,
}

#[protein]
pub struct TransferItem {
    pub from_player: u64,
    pub to_player: u64,
    pub item_id: String,
    pub quantity: u64,
}

#[protein]
pub struct GetInventory {
    pub player_id: u64,
}

#[protein]
pub struct InventorySnapshot {
    pub player_id: u64,
    pub items: Vec<InventoryEntry>,
    pub capacity: u32,
    pub used_slots: u32,
}

#[protein]
pub struct GetItemDefinition {
    pub item_id: String,
}

#[protein]
pub struct RegisterItemDefinition {
    pub item: Item,
}

#[protein]
pub struct ListItemDefinitions {
    pub category: Option<ItemCategory>,
}

#[protein]
pub struct CraftItem {
    pub player_id: u64,
    pub recipe_id: String,
    pub quantity: u32,
}

#[protein]
pub struct Recipe {
    pub id: String,
    pub output_item_id: String,
    pub output_quantity: u32,
    pub inputs: Vec<RecipeIngredient>,
    pub duration_ms: u64,
    pub required_level: u32,
}

#[protein]
pub struct RecipeIngredient {
    pub item_id: String,
    pub quantity: u32,
}

#[protein]
pub struct Ping;

// ========= SERVICE =========

#[service]
#[derive(Clone)]
struct InventoryService {
    db: Arc<Mutex<Connection>>,
    db_path: PathBuf,
}

impl InventoryService {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
        ")?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS item_definitions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                category INTEGER NOT NULL,
                rarity INTEGER NOT NULL,
                mass REAL NOT NULL,
                volume REAL NOT NULL,
                base_value INTEGER NOT NULL,
                max_stack INTEGER NOT NULL,
                properties TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS inventories (
                player_id INTEGER PRIMARY KEY,
                capacity INTEGER NOT NULL DEFAULT 100,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS inventory_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                player_id INTEGER NOT NULL,
                item_id TEXT NOT NULL,
                quantity INTEGER NOT NULL,
                durability REAL DEFAULT 1.0,
                custom_data TEXT,
                slot INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (player_id) REFERENCES inventories(player_id) ON DELETE CASCADE,
                FOREIGN KEY (item_id) REFERENCES item_definitions(id),
                UNIQUE(player_id, slot) ON CONFLICT ABORT
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recipes (
                id TEXT PRIMARY KEY,
                output_item_id TEXT NOT NULL,
                output_quantity INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                required_level INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (output_item_id) REFERENCES item_definitions(id)
            )",
            [],
        )?;
        
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recipe_ingredients (
                recipe_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                quantity INTEGER NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (recipe_id, position),
                FOREIGN KEY (recipe_id) REFERENCES recipes(id) ON DELETE CASCADE,
                FOREIGN KEY (item_id) REFERENCES item_definitions(id)
            )",
            [],
        )?;
        
        conn.execute("CREATE INDEX IF NOT EXISTS idx_inventory_items_player ON inventory_items(player_id)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_inventory_items_item ON inventory_items(item_id)", [])?;
        conn.execute("CREATE INDEX IF NOT EXISTS idx_recipes_output ON recipes(output_item_id)", [])?;
        
        Ok(())
    }
}

#[handler]
impl InventoryService {
    async fn ping(&self, _req: Ping) -> Result<()> {
        Ok(())
    }
    
    async fn register_item_definition(&self, req: RegisterItemDefinition) -> Result<()> {
        let conn = self.db.lock().await;
        
        let properties_json = serde_json::to_string(&req.item.properties)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        conn.execute(
            "INSERT OR REPLACE INTO item_definitions 
             (id, name, description, category, rarity, mass, volume, base_value, max_stack, properties, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                req.item.id,
                req.item.name,
                req.item.description,
                req.item.category as i32,
                req.item.rarity as i32,
                req.item.mass,
                req.item.volume,
                req.item.base_value,
                req.item.max_stack,
                properties_json,
                now,
            ],
        )?;
        
        Ok(())
    }
    
    async fn get_item_definition(&self, req: GetItemDefinition) -> Result<Option<Item>> {
        let conn = self.db.lock().await;
        
        let mut stmt = conn.prepare(
            "SELECT id, name, description, category, rarity, mass, volume, base_value, max_stack, properties 
             FROM item_definitions WHERE id = ?"
        )?;
        
        let item = stmt.query_row([&req.item_id], |row| {
            Self::map_item_row(row)
        }).optional()?;
        
        Ok(item)
    }
    
    async fn list_item_definitions(&self, req: ListItemDefinitions) -> Result<Vec<Item>> {
        let conn = self.db.lock().await;
        
        let mut query = "SELECT id, name, description, category, rarity, mass, volume, base_value, max_stack, properties 
                        FROM item_definitions".to_string();
        
        let category_value = req.category;
        if category_value.is_some() {
            query.push_str(" WHERE category = ?");
        }
        
        let mut stmt = conn.prepare(&query)?;
        
        // Use function pointer Self::map_item_row directly to avoid closure type mismatch
        let items = if let Some(category) = category_value {
            stmt.query_map([category as i32], Self::map_item_row)?
        } else {
            stmt.query_map([], Self::map_item_row)?
        };
        
        let mut result = Vec::new();
        for item in items {
            result.push(item?);
        }
        
        Ok(result)
    }
    
    async fn create_inventory(&self, req: CreateInventory) -> Result<()> {
        let conn = self.db.lock().await;
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        
        conn.execute(
            "INSERT OR REPLACE INTO inventories (player_id, capacity, created_at, updated_at)
             VALUES (?, ?, ?, ?)",
            params![req.player_id, req.capacity, now, now],
        )?;
        
        Ok(())
    }
    
    async fn deposit_item(&self, req: DepositItem) -> Result<InventorySnapshot> {
        let player_id = req.player_id;
        
        {
            let conn = self.db.lock().await;
            
            let tx = conn.unchecked_transaction()?;
            
            let exists: bool = tx.query_row(
                "SELECT 1 FROM inventories WHERE player_id = ?",
                [player_id],
                |_| Ok(true),
            ).unwrap_or(false);
            
            if !exists {
                tx.execute(
                    "INSERT INTO inventories (player_id, capacity, created_at, updated_at) VALUES (?, 100, ?, ?)",
                    params![
                        player_id,
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64,
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64,
                    ],
                )?;
            }
            
            let existing: Option<(i64, u64)> = tx.query_row(
                "SELECT id, quantity FROM inventory_items 
                 WHERE player_id = ? AND item_id = ? AND (custom_data IS NULL OR custom_data = ?)",
                params![player_id, req.item_id, req.custom_data],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            
            if let Some((row_id, current_qty)) = existing {
                let new_qty = current_qty + req.quantity;
                tx.execute(
                    "UPDATE inventory_items SET quantity = ?, updated_at = ? WHERE id = ?",
                    params![new_qty, now, row_id],
                )?;
            } else {
                let used_slots_str: Option<String> = tx.query_row(
                    "SELECT GROUP_CONCAT(slot) FROM inventory_items WHERE player_id = ? AND slot IS NOT NULL",
                    [player_id],
                    |row| row.get(0),
                ).unwrap_or(None);
                
                let used_slots: Vec<u32> = used_slots_str
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|s| s.parse::<u32>().ok())
                    .collect();
                
                let slot = (1..=100).find(|s| !used_slots.contains(s));
                
                tx.execute(
                    "INSERT INTO inventory_items 
                     (player_id, item_id, quantity, durability, custom_data, slot, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        player_id,
                        req.item_id,
                        req.quantity,
                        req.durability.unwrap_or(1.0),
                        req.custom_data,
                        slot,
                        now,
                        now,
                    ],
                )?;
            }
            
            tx.commit()?;
        }
        
        self.get_inventory(GetInventory { player_id }).await
    }
    
    async fn withdraw_item(&self, req: WithdrawItem) -> Result<InventorySnapshot> {
        let player_id = req.player_id;
        
        {
            let conn = self.db.lock().await;
            
            let tx = conn.unchecked_transaction()?;
            
            let current_qty: u64 = tx.query_row(
                "SELECT COALESCE(SUM(quantity), 0) FROM inventory_items 
                 WHERE player_id = ? AND item_id = ?",
                params![player_id, req.item_id],
                |row| row.get::<_, u64>(0),
            ).unwrap_or(0);
            
            if current_qty < req.quantity {
                return Err(anyhow::anyhow!("Insufficient quantity"));
            }
            
            let mut remaining = req.quantity;
            
            let mut stmt = tx.prepare(
                "SELECT id, quantity FROM inventory_items 
                 WHERE player_id = ? AND item_id = ? 
                 ORDER BY created_at ASC"
            )?;
            
            let rows: Vec<(i64, u64)> = stmt.query_map(params![player_id, req.item_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?.collect::<Result<Vec<_>, _>>()?;
            
            drop(stmt);
            
            for (row_id, qty) in rows {
                if remaining == 0 { break; }
                
                if qty <= remaining {
                    tx.execute("DELETE FROM inventory_items WHERE id = ?", [row_id])?;
                    remaining -= qty;
                } else {
                    let new_qty = qty - remaining;
                    tx.execute(
                        "UPDATE inventory_items SET quantity = ? WHERE id = ?",
                        params![new_qty, row_id],
                    )?;
                    remaining = 0;
                }
            }
            
            tx.commit()?;
        }
        
        self.get_inventory(GetInventory { player_id }).await
    }
    
    async fn transfer_item(&self, req: TransferItem) -> Result<InventorySnapshot> {
        self.withdraw_item(WithdrawItem {
            player_id: req.from_player,
            item_id: req.item_id.clone(),
            quantity: req.quantity,
        }).await?;
        
        self.deposit_item(DepositItem {
            player_id: req.to_player,
            item_id: req.item_id,
            quantity: req.quantity,
            durability: None,
            custom_data: None,
        }).await
    }
    
    async fn get_inventory(&self, req: GetInventory) -> Result<InventorySnapshot> {
        let conn = self.db.lock().await;
        
        let capacity: u32 = conn.query_row(
            "SELECT capacity FROM inventories WHERE player_id = ?",
            [req.player_id],
            |row| row.get(0),
        ).unwrap_or(100);
        
        let mut stmt = conn.prepare(
            "SELECT item_id, quantity, durability, custom_data, slot 
             FROM inventory_items 
             WHERE player_id = ?
             ORDER BY slot NULLS LAST, created_at"
        )?;
        
        let items = stmt.query_map([req.player_id], |row| {
            Ok(InventoryEntry {
                player_id: req.player_id,
                item_id: row.get(0)?,
                quantity: row.get(1)?,
                durability: row.get(2)?,
                custom_data: row.get(3)?,
                slot: row.get(4)?,
            })
        })?;
        
        let mut item_vec = Vec::new();
        let mut used_slots = 0;
        
        for item in items {
            let entry = item?;
            if entry.slot.is_some() {
                used_slots += 1;
            }
            item_vec.push(entry);
        }
        
        Ok(InventorySnapshot {
            player_id: req.player_id,
            items: item_vec,
            capacity,
            used_slots,
        })
    }
    
    async fn craft_item(&self, req: CraftItem) -> Result<InventorySnapshot> {
        let recipe = self.get_recipe(req.recipe_id.clone()).await?;
        let player_id = req.player_id;
        
        {
            let conn = self.db.lock().await;
            
            for ingredient in &recipe.inputs {
                let qty: u64 = conn.query_row(
                    "SELECT COALESCE(SUM(quantity), 0) FROM inventory_items 
                     WHERE player_id = ? AND item_id = ?",
                    params![player_id, ingredient.item_id],
                    |row| row.get::<_, u64>(0),
                ).unwrap_or(0);
                
                if qty < ingredient.quantity as u64 {
                    return Err(anyhow::anyhow!(
                        "Insufficient {}: need {}, have {}",
                        ingredient.item_id,
                        ingredient.quantity,
                        qty
                    ));
                }
            }
        }
        
        for ingredient in &recipe.inputs {
            self.withdraw_item(WithdrawItem {
                player_id,
                item_id: ingredient.item_id.clone(),
                quantity: ingredient.quantity as u64,
            }).await?;
        }
        
        self.deposit_item(DepositItem {
            player_id,
            item_id: recipe.output_item_id,
            quantity: recipe.output_quantity as u64,
            durability: None,
            custom_data: None,
        }).await
    }
    
    async fn get_recipe(&self, recipe_id: String) -> Result<Recipe> {
        let conn = self.db.lock().await;
        
        let mut recipe = conn.query_row(
            "SELECT output_item_id, output_quantity, duration_ms, required_level 
             FROM recipes WHERE id = ?",
            [&recipe_id],
            |row| {
                Ok(Recipe {
                    id: recipe_id.clone(),
                    output_item_id: row.get(0)?,
                    output_quantity: row.get(1)?,
                    duration_ms: row.get(2)?,
                    required_level: row.get(3)?,
                    inputs: Vec::new(),
                })
            },
        )?;
        
        let mut stmt = conn.prepare(
            "SELECT item_id, quantity FROM recipe_ingredients 
             WHERE recipe_id = ? ORDER BY position"
        )?;
        
        let ingredients = stmt.query_map([&recipe_id], |row| {
            Ok(RecipeIngredient {
                item_id: row.get(0)?,
                quantity: row.get(1)?,
            })
        })?;
        
        for ingredient in ingredients {
            recipe.inputs.push(ingredient?);
        }
        
        Ok(recipe)
    }
}

impl From<i32> for ItemCategory {
    fn from(v: i32) -> Self {
        match v {
            0 => ItemCategory::Ore,
            1 => ItemCategory::Refined,
            2 => ItemCategory::Component,
            3 => ItemCategory::Tool,
            4 => ItemCategory::Weapon,
            5 => ItemCategory::Thruster,
            6 => ItemCategory::Reactor,
            7 => ItemCategory::Cargo,
            8 => ItemCategory::Drill,
            9 => ItemCategory::Scanner,
            10 => ItemCategory::Shield,
            11 => ItemCategory::Utility,
            12 => ItemCategory::Consumable,
            13 => ItemCategory::Quest,
            _ => ItemCategory::Ore,
        }
    }
}

impl From<i32> for Rarity {
    fn from(v: i32) -> Self {
        match v {
            0 => Rarity::Common,
            1 => Rarity::Uncommon,
            2 => Rarity::Rare,
            3 => Rarity::Epic,
            4 => Rarity::Legendary,
            5 => Rarity::Mythic,
            _ => Rarity::Common,
        }
    }
}

impl InventoryService {
    fn map_item_row(row: &rusqlite::Row) -> Result<Item, rusqlite::Error> {
        let properties_json: String = row.get(9)?;
        let properties: Vec<(String, String)> = serde_json::from_str(&properties_json)
            .unwrap_or_default();
        
        Ok(Item {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            category: row.get::<_, i32>(3)?.into(),
            rarity: row.get::<_, i32>(4)?.into(),
            mass: row.get(5)?,
            volume: row.get(6)?,
            base_value: row.get(7)?,
            max_stack: row.get(8)?,
            properties,
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(true)
        .init();
    
    println!("📦 Inventory Cell - SQLite Persistence");
    println!("   └─ Database: WAL mode + Foreign Keys");
    
    let home = dirs::home_dir().expect("No HOME directory");
    let db_dir = home.join(".cell/data/inventory");
    std::fs::create_dir_all(&db_dir)?;
    let db_path = db_dir.join("inventory.db");
    
    let service = InventoryService {
        db: Arc::new(Mutex::new(Connection::open(&db_path)?)),
        db_path,
    };
    
    service.init_db()?;
    
    let items = service.list_item_definitions(ListItemDefinitions { category: None }).await?;
    if items.is_empty() {
        println!("   └─ Seeding default items...");
        
        service.register_item_definition(RegisterItemDefinition {
            item: Item {
                id: "ore_iron".to_string(),
                name: "Iron Ore".to_string(),
                description: "Raw iron ore, requires smelting".to_string(),
                category: ItemCategory::Ore,
                rarity: Rarity::Common,
                mass: 1.0,
                volume: 0.5,
                base_value: 10,
                max_stack: 100,
                properties: Vec::new(),
            },
        }).await?;
        
        service.register_item_definition(RegisterItemDefinition {
            item: Item {
                id: "ore_copper".to_string(),
                name: "Copper Ore".to_string(),
                description: "Raw copper ore".to_string(),
                category: ItemCategory::Ore,
                rarity: Rarity::Common,
                mass: 1.0,
                volume: 0.5,
                base_value: 12,
                max_stack: 100,
                properties: Vec::new(),
            },
        }).await?;
        
        service.register_item_definition(RegisterItemDefinition {
            item: Item {
                id: "plate_iron".to_string(),
                name: "Iron Plate".to_string(),
                description: "Smelted iron, used in construction".to_string(),
                category: ItemCategory::Refined,
                rarity: Rarity::Common,
                mass: 0.5,
                volume: 0.2,
                base_value: 25,
                max_stack: 200,
                properties: Vec::new(),
            },
        }).await?;
        
        println!("   └─ ✓ Default items seeded");
    }
    
    service.serve("inventory").await
}