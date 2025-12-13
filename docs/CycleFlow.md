graph TB
    subgraph "Phase 1: Bootstrap"
        A[User: cell up] --> B[Control Plane Start]
        B --> C{State File Exists?}
        C -->|Yes| D[Validate PIDs]
        C -->|No| E[Fresh Start]
        D --> F[Kill Stale Processes]
        E --> G[Sequential Kernel Boot]
        F --> G
        G --> G1[1. Builder]
        G1 --> G2[2. Hypervisor]
        G2 --> G3[3. Nucleus]
        G3 --> G4[4. Mesh]
        G4 --> G5[5. Axon]
        G5 --> G6[6. Observer]
        G6 --> H[Persist State]
    end

    subgraph "Phase 2: Application Startup"
        H --> I[Query Mesh for Dependencies]
        I --> J[Topological Sort]
        J --> K{For Each Cell}
        K --> L{Already Running?}
        L -->|Yes| M{Version Match?}
        L -->|No| N[Spawn via Hypervisor]
        M -->|Yes| O[Skip]
        M -->|No| P[Trigger Hot-Swap]
        N --> Q[Wait for Cytokinesis]
        Q --> R[Register in Nucleus]
        P --> S[Swap Coordinator]
        R --> T[Update State]
        S --> T
        T --> K
    end

    subgraph "Phase 3: Monitoring"
        T --> U[Health Loop Every 5s]
        U --> V{Check All Cells}
        V --> W{PID Alive?}
        W -->|No| X[Log: Cell Died]
        W -->|Yes| Y{Socket Responsive?}
        Y -->|No| X
        Y -->|Yes| Z{Nucleus Heartbeat OK?}
        Z -->|No| X
        Z -->|Yes| AA[Mark Healthy]
        X --> AB[Restart Cell with Backoff]
        AB --> AC[Update State]
        AC --> U
        AA --> U
        
        U --> AD[Version Loop Every 60s]
        AD --> AE{Check Source Hashes}
        AE --> AF{Outdated?}
        AF -->|Yes| AG[Log: Update Available]
        AF -->|No| AD
        AG --> AD
    end

    subgraph "Phase 4: Hot-Swap"
        AH[User: cell swap] --> AI[Swap Coordinator]
        AI --> AJ[Query Builder for New Hash]
        AJ --> AK[Compile New Binary]
        AK --> AL[Spawn at Temp Socket]
        AL --> AM{Health Check Pass?}
        AM -->|No| AN[Rollback: Keep Old]
        AM -->|Yes| AO[Send Shutdown to Old]
        AO --> AP[Wait for Graceful Drain]
        AP --> AQ[Atomic Socket Rename]
        AQ --> AR[Update Routing Tables]
        AR --> AS[Kill Old Instance]
        AS --> AT[Update State]
    end

    subgraph "Phase 5: Shutdown"
        AU[User: cell down] --> AV[Control Plane Shutdown]
        AV --> AW[Read Dependency Graph]
        AW --> AX[Reverse Topological Sort]
        AX --> AY{For Each Cell Leaf-First}
        AY --> AZ[Send OPS::Shutdown]
        AZ --> BA{Exit in 5s?}
        BA -->|Yes| BB[Clean Exit]
        BA -->|No| BC[SIGTERM]
        BC --> BD{Exit in 2s?}
        BD -->|Yes| BB
        BD -->|No| BE[SIGKILL]
        BE --> BB
        BB --> BF{More Cells?}
        BF -->|Yes| AY
        BF -->|No| BG[Shutdown Kernel Cells]
        BG --> BH[Update State: All Stopped]
        BH --> BI[Exit Control Plane]
    end

    style A fill:#4CAF50
    style B fill:#2196F3
    style AH fill:#FF9800
    style AU fill:#F44336
    style U fill:#9C27B0
    style AI fill:#FF9800