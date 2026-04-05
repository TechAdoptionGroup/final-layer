# Protocol Upgrade Procedure

## Overview

Final Layer inherits NEAR Protocol's protocol versioning system. Each consensus-critical change (gas constants, new opcodes, changed behavior) requires a **hard fork** — a coordinated upgrade where all validators must switch to the new binary simultaneously.

---

## Protocol Version History

| Version | Type | Changes |
|---|---|---|
| v1001 | Genesis | PQC cryptography, 9-shard genesis config |
| v1002 | Hard fork | Multi-shard epoch config (`epoch_configs/1002.json`) |
| v1003 | Hard fork | PQC gas rebalance: ML-DSA 2.1→3.0 TGas, SLH-DSA 3.2→8.0 TGas |

---

## Hard Fork Procedure

### When is a hard fork required?

A hard fork is required when changing any **consensus-critical** value:
- Gas constants for host functions
- New WASM opcodes or host functions
- Changes to state transition logic
- Protocol version gating

Gas constants are consensus-critical because all validators must agree on the gas cost of every operation. If two validators use different gas constants, they produce different state roots for the same block → chain split.

### Steps

**1. Patch the source**

For gas constant changes, edit `runtime/near-vm-runner/src/logic/pqc_host_fns.rs`:
```rust
const MLDSA_VERIFY_BASE_GAS: u64 = 3_000_000_000_000;  // 3.0 TGas
const SLHDSA_VERIFY_BASE_GAS: u64 = 8_000_000_000_000; // 8.0 TGas
const FNDSA_VERIFY_BASE_GAS: u64 = 1_400_000_000_000;  // 1.4 TGas (unchanged)
```

**2. Bump the protocol version**

Edit `core/primitives-core/src/version.rs`:
```rust
const STABLE_PROTOCOL_VERSION: ProtocolVersion = 1003; // increment from previous
```

**3. Build**
```bash
cd /path/to/nearcore
cargo build --release -p neard
# Produces target/release/neard (~92MB)
```

**4. Distribute binary to all validators**

All validators must receive the new binary before any are restarted. Use atomic file replacement to avoid "text file busy" errors on running binaries:
```bash
install -m 755 /path/to/neard_new /usr/local/bin/neard
```

**5. Coordinated restart**

Stop all validators, then restart simultaneously:
```bash
systemctl stop fl-node
# (after all nodes confirmed stopped)
systemctl start fl-node
```

**6. Verify**

Check that all nodes are running the new binary and syncing. The chain protocol version upgrades automatically at the next epoch boundary once 2/3+ of validators vote for the new version.

---

## Automatic Protocol Activation

When all validators are running a binary that supports protocol version N+1:
1. Validators include their supported version in block headers
2. At an epoch boundary, if ≥ 2/3 of stake has voted for N+1, the chain upgrades
3. No manual intervention required after the binary swap

The on-chain protocol version (`chain_proto` in RPC status) will show the old version until the epoch boundary. This is expected — the binary version and chain protocol version are different things.

---

## Backup Strategy

Before any upgrade, back up the current binary:
```bash
cp /usr/local/bin/neard /usr/local/bin/neard_v<PREV_VERSION>_backup
```

In case of issues, restore and restart:
```bash
install -m 755 /usr/local/bin/neard_v1002_backup /usr/local/bin/neard
systemctl start fl-node
```
