| Cell               | Own repo         | Ships as               | Reason                                                                                                    |
| ------------------ | ---------------- | ---------------------- | --------------------------------------------------------------------------------------------------------- |
| **cell-spec**      | `cell-spec`      | git submodule + crate  | The JSON schemas, protobuf, or Cap’n-Proto IDL that every language binds against. Single source of truth. |
| **cell-sdk-rust**  | `cell-sdk-rust`  | crates.io              | Zero-copy Rust SDK + macros.                                                                              |
| **cell-sdk-py**    | `cell-sdk-py`    | PyPI                   | Python membrane + synapse.                                                                                |
| **cell-consensus** | `cell-consensus` | crates.io              | WAL + network; usable outside the ecosystem.                                                              |
| **cell-cli**       | `cell-cli`       | GitHub release tarball | Daemon, Golgi, Mitochondria; binaries.                                                                    |
| **cell-raft-kv**   | `cell-raft-kv`   | Docker image           | Example application, not framework code.                                                                  |
| **cell-bench**     | `cell-bench`     | GitHub release         | Benchmark suite; depends on `cell-sdk-rust` crate, not source.                                            |
