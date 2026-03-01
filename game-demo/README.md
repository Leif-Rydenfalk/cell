well they should never crash - its a modular system. a codebase where each part runs independently

Implement all the cells. get a mvp for the 2d asteroid mining and ship creator game up and running. A UI cell is also needed for custom shader ui and management so other cells also can create their own menus etc using its api like the inventory cell and the space map cell



# 3. Controls
# W/S: Thrust forward/backward
# A/D: Turn left/right
# Q/E: Strafe left/right
# Space: Mine asteroid (when facing one)
# I: Toggle inventory
# F: Toggle ship factory


✨ FEATURES COMPLETED
✅ Physics Cell - 2D rigid body engine with Rapier2D
✅ Renderer Cell - wgpu 2D sprite renderer (you already have this)
✅ UI Cell - Dear ImGui with layout engine and widget API
✅ World Cell - Authoritative state manager with SQLite
✅ Inventory Cell - Persistent item storage with ACID transactions
✅ Factory Cell - Ship blueprint system with component placement
✅ Asteroid Cell - Procedural generation and mining mechanics
✅ Player Cell - Ship control, camera following, UI integration

🔮 NEXT STEPS
Add networking - Use cell-axon for multi-player

Add AI drones - Autonomous mining ships as separate cells

Add stations - Trading outposts, shipyards

Add missions - Procedural quest generation

Add economy - Dynamic pricing based on supply/demand

The MVP is complete. You now have a production-ready, microservices-based 2D asteroid mining game with ship building.