# Final Layer Staking Pool — Complete Bug History (v1 → v9)

**Contract:** `fl_staking_pool` (deployed to each validator account as a NEAR/FLC smart contract)  
**Current version:** v9  
**Date of full resolution:** 2026-04-06

---

## Overview

The Final Layer staking pool went through nine iterations to reach a fully correct state. Eight distinct bugs were identified and fixed across those versions. This document explains each bug's root cause, how it manifested, and what the fix was.

---

## Bug 1 — Wrong Key Type Bytes (v1 → v3)

### What broke
Every call to `deposit_and_stake()` appeared to succeed at the contract level but silently failed when the contract tried to call `promise_batch_action_stake()`. The validator's staked balance never changed on-chain.

### Root cause
`parse_key_string()` serializes a PQC public key into the Borsh format expected by `near_crypto::PublicKey`. The first byte is the `KeyType` discriminant, which must exactly match:

```
near_crypto::KeyType:
  ED25519   = 0
  SECP256K1 = 1
  MLDSA     = 2
  FNDSA     = 3
  SLHDSA    = 4
```

The v1 contract assigned the wrong values:

```rust
// v1 — WRONG (collides with ED25519=0 and SECP256K1=1)
"mldsa"  => 0,
"fndsa"  => 1,
"slhdsa" => 2,
```

When the runtime received an ML-DSA key encoded with type byte `0`, it tried to read it as an ED25519 key. ED25519 keys are 32 bytes; an ML-DSA key is 1952 bytes. BorshDeserialize immediately rejected the malformed payload, causing `promise_batch_action_stake` to panic and revert the entire transaction.

### Fix (v3)
Corrected the key type bytes to match `near_crypto::KeyType`:

```rust
// v3 — CORRECT
"mldsa"  => 2,
"fndsa"  => 3,
"slhdsa" => 4,
```

---

## Bug 2 — Missing Borsh Vec\<u8\> Length Prefix (v3 → v4)

### What broke
After fixing the key type bytes in v3, `deposit_and_stake()` still failed with a BorshDeserialize error in `promise_batch_action_stake()`.

### Root cause
PQC public key structs in `near_crypto` use `Vec<u8>` as their inner type. When Borsh serializes a `Vec<u8>`, it writes a 4-byte little-endian length prefix before the data. The v3 code omitted this prefix:

```
v3 (wrong):  [type_byte(1)] + [raw_key_bytes(897)]     = 898 bytes
v4 (correct): [type_byte(1)] + [len_LE_u32(4)] + [raw_key_bytes(897)] = 902 bytes
```

The runtime's Borsh decoder read the first 4 bytes of the key as the `Vec<u8>` length field, got a garbage value, then tried to read that many bytes as the key — panicking and reverting.

Concrete values for each PQC type:
- **FN-DSA (Falcon-512):** `[0x03, 0x81,0x03,0x00,0x00, 897_bytes]` = 902 bytes
- **ML-DSA (Dilithium3):** `[0x02, 0xC0,0x07,0x00,0x00, 1952_bytes]` = 1957 bytes
- **SLH-DSA (SPHINCS+):** `[0x04, 0x20,0x00,0x00,0x00, 32_bytes]` = 37 bytes

### Fix (v4)
Added the 4-byte Borsh length prefix for PQC keys:

```rust
match algo {
    "mldsa" | "fndsa" | "slhdsa" => {
        let len_bytes = (key_bytes.len() as u32).to_le_bytes();
        result.extend_from_slice(&len_bytes);
    }
    _ => {}
}
```

---

## Bug 3 — Bootstrap Share Price Inflation (v4 → v5)

### What broke
The very first delegator to deposit into a freshly deployed pool instantly received a massively inflated share value. A deposit of 1,000 FLC would appear to give the user 300 million FLC worth of shares — equal to the validator's entire self-stake.

### Root cause
On the first deposit, `internal_ping()` computes the reward delta:

```rust
let locked = env::account_locked_balance();
if locked > self.last_locked_balance {
    self.total_staked_balance += locked - self.last_locked_balance;
}
```

At genesis, `last_locked_balance = 0` but `locked = 300,000,000 FLC` (validator's pre-existing self-stake). So `internal_ping()` credited all 300 million FLC as "rewards" to the pool before a single delegator share existed. The first depositor then received shares calculated against this inflated `total_staked_balance`, giving them an astronomical share price.

### Fix (v5)
Sync `last_locked_balance` to the current protocol-locked amount before creating the first shares:

```rust
pub fn deposit_and_stake(&mut self) {
    // Sync before the first deposit to prevent validator's pre-existing
    // locked balance from being credited as phantom rewards.
    if self.total_stake_shares == 0 {
        self.last_locked_balance = env::account_locked_balance().as_yoctonear();
    }
    self.internal_ping();
    // ...
}
```

---

## Bug 4 — Wrong-Size Key Panic Instead of Silent Corruption (v4 → v5)

### What broke
A key with a truncated or padded base58 encoding would silently produce a wrong-length byte slice that the runtime might accept or reject non-deterministically.

### Fix (v5)
Added exact-length validation in `parse_key_string()`:

```rust
if key_bytes.len() != pk_len {
    panic!("Key must be exactly {} bytes for {}, got {}", pk_len, algo, key_bytes.len());
}
```

---

## Bug 5 — Phantom Restake Reward (v5 → v6)

### What broke
After any action that called `internal_restake()`, the very next `internal_ping()` would see an enormous phantom reward equal to the validator's entire self-stake (e.g. 300 million FLC), crediting it to all delegators and inflating the share price by orders of magnitude.

### Root cause
The v5 code updated `last_locked_balance` inside `internal_restake()`:

```rust
fn internal_restake(&mut self) {
    let amt = self.total_staked_balance;
    // ... issue stake(amt)
    self.last_locked_balance = amt;  // BUG: amt is delegator total, ~1,111 FLC
}
```

This set `last_locked_balance` to the delegator total (e.g. 1,111 FLC). At the next epoch, the protocol had not yet applied the new stake action, so `env::account_locked_balance()` still returned 300,000,000 FLC. The next `internal_ping()` saw:

```
locked (300,000,000) - last_locked (1,111) = 299,998,889 FLC "rewards"
```

…crediting nearly 300 million FLC as phantom rewards.

### Fix (v6)
Removed the `self.last_locked_balance = amt` line from `internal_restake()`. Only `internal_ping()` is responsible for tracking locked balance changes.

---

## Bug 6 — u128 Overflow in Share Arithmetic (v6 → v7)

### What broke
At yoctoFLC scale (1 FLC = 10²⁴ yoctoFLC), intermediate calculations in share math could silently overflow u128, producing incorrect share values.

For example:  
`shares * total_staked_balance` at 300,000 FLC = 3×10⁵ × 10²⁴ × 3×10⁵ × 10²⁴ = 9×10⁵² » u128 max (~3.4×10³⁸)

### Fix (v7)
Added overflow-safe `muldiv128(a, b, c)` that computes `floor(a*b/c)` without overflow:

```rust
fn muldiv128(a: u128, b: u128, c: u128) -> u128 {
    if c == 0 { return 0; }
    if let Some(ab) = a.checked_mul(b) { return ab / c; }
    // Decompose: floor(a*b/c) = (a/c)*b + floor((a%c)*b/c)
    let q = a / c;
    let r = a % c;
    q.saturating_mul(b).saturating_add(
        if let Some(rb) = r.checked_mul(b) { rb / c } else {
            r.saturating_mul(b / c).saturating_add(r.saturating_mul(b % c) / c)
        }
    )
}
```

All share calculations (`shares_for_amount`, `amount_for_shares`, `shares_for_amount_post_reduce`) were updated to use `muldiv128`.

---

## Bug 7 — Admin Recovery Functions Missing (v7 → v8)

### What broke
After accounting corruption events (e.g. the phantom reward bug before v6 was applied), there was no way to surgically correct the pool's internal state without a full re-deployment and re-initialization.

### Fix (v8)
Added owner-only emergency correction functions:

```rust
pub fn fix_pool_accounting(&mut self, total_staked: U128, last_locked: U128)
pub fn fix_delegator_shares(&mut self, account_id: AccountId, shares: U128, principal: U128)
pub fn fix_delegator_unstaked(&mut self, account_id: AccountId, unstaked_balance: U128)
pub fn debug_get_stake_shares(&self, account_id: AccountId) -> String
pub fn debug_amount_for_shares(&self, shares: U128) -> U128
```

These allowed correcting corrupted pool state without redeployment or loss of user funds.

---

## Bug 8 — Validator Stake Drop on internal_restake (v8 → v9)

### What broke
When a validator has both a personal self-stake (staked directly through the protocol, not via the pool) and delegator funds managed by the pool contract, calling any pool action that triggers `internal_restake()` would drop the validator's stake to only the delegator total, potentially losing hundreds of millions of FLC from the active validator set.

**Example scenario:**
- Validator's protocol-locked balance: 300,000,000 FLC (self-stake established at genesis)  
- Pool's `total_staked_balance`: 1,111 FLC (delegator deposits)  
- When `internal_restake()` fired: `stake(1,111 FLC)` → validator's stake drops from 300M to 1,111 FLC

This would have caused the validator to effectively exit the active set (or receive vastly reduced voting weight), disrupting chain finality.

The same risk applied to any fresh pool deployment where `total_staked_balance = 0` and the validator had existing self-stake: `stake(0)` would fully unstake the validator at the next epoch boundary.

### Root cause
`internal_restake()` used `self.total_staked_balance` directly as the stake amount:

```rust
// v8 — UNSAFE
fn internal_restake(&mut self) {
    let amt = self.total_staked_balance;  // only delegator funds, not self-stake
    // ... issue stake(amt)
    // If validator has 300M self-stake and pool has 1,111 FLC delegated:
    // stake(1,111 FLC) → drops validator from 300M to 1,111 FLC at next epoch
}
```

### Fix (v9)
Take the maximum of `total_staked_balance` and `last_locked_balance` to guarantee the issued stake amount is never lower than the current protocol-locked balance:

```rust
// v9 — CORRECT
fn internal_restake(&mut self) {
    if self.staking_key_bytes.is_empty() { return; }
    #[cfg(target_arch = "wasm32")]
    {
        let acct = env::current_account_id().to_string();
        // Never issue a stake action smaller than what's currently locked.
        // This prevents self-stake from being inadvertently reduced when
        // total_staked_balance (delegator total) < last_locked_balance (full validator lock).
        let amt = self.total_staked_balance.max(self.last_locked_balance);
        let pk  = self.staking_key_bytes.clone();
        unsafe {
            let idx = sys_promise_batch_create(&acct);
            sys_promise_batch_action_stake(idx, amt, &pk);
        }
    }
}
```

### Recovery procedure
When v7/v8 pools were discovered with this vulnerability, the following steps were applied to each affected validator:

1. **Deploy v9 WASM** to the pool account (upgrades the contract in-place, preserving state)
2. **Call `fix_pool_accounting`** to set `last_locked` to the actual protocol-locked balance, ensuring `internal_restake` uses the correct maximum
3. **Call `update_staking_key`** to trigger `internal_restake` with the correct max amount, immediately re-staking at the full validator stake level

---

## Version Summary

| Version | Key Change | Status |
|---------|-----------|--------|
| v1 | Genesis deploy — wrong key type bytes (mldsa=0,fndsa=1,slhdsa=2) | BROKEN |
| v2 | Temporary no-op `internal_restake()` workaround | PARTIAL |
| v3 | Correct key type bytes (mldsa=2,fndsa=3,slhdsa=4) | BROKEN (no length prefix) |
| v4 | Added Borsh Vec\<u8\> 4-byte length prefix for PQC keys | WORKING |
| v5 | Bootstrap fix (sync last_locked before first shares); exact-length key validation | WORKING |
| v6 | Removed phantom-reward-causing `last_locked = amt` from `internal_restake()` | WORKING |
| v7 | `muldiv128()` overflow-safe share math | WORKING |
| v8 | Admin emergency recovery functions (`fix_pool_accounting`, `fix_delegator_shares`, etc.) | WORKING |
| v9 | `internal_restake` uses `max(total_staked, last_locked)` to prevent validator stake drop | **CURRENT** ✓ |

---

## Deployment Checklist for New Pools

When deploying a staking pool to a new validator account:

1. Deploy the v9 WASM: `fl-send-tx deploy --wasm fl_staking_pool.wasm --receiver <validator.fl>`
2. Call `fix_pool_accounting(total_staked="0", last_locked="<current_locked_yocto>")` if the validator has pre-existing self-stake
3. Call `update_staking_key(new_staking_key="<algo>:<base58_pubkey>")` to set the consensus key and trigger the first restake at the correct amount

This order is critical: `fix_pool_accounting` must be called **before** `update_staking_key` (which triggers `internal_restake`). If the order is reversed, `internal_restake` fires with `max(0, 0) = 0` and fully unstakes the validator.
