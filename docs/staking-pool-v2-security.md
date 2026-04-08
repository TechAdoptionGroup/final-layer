# Staking Pool v2 — Security Hardening Changelog

*Deployed: 2026-04-08 | Audit: HashLock AI (hashlock.com)*

## Summary of Changes

This release hardens the staking pool contract against malicious or compromised
pool operators. The core principle: **delegators must always be able to unstake
and withdraw their principal**, regardless of validator actions.

---

## HIGH Severity Fixes

### H1 — Staking Key Bytes Validation (internal_restake)
**Before:** Raw PQC key bytes were passed to the unsafe WASM host function
without verifying the stored bytes match a known valid PQC key encoding.

**After:** `internal_restake()` checks the length of `staking_key_bytes`
against the three known PQC Borsh-encoded sizes (FNDSA=902, MLDSA=1957,
SLHDSA=37). If the length doesn't match, the restake is skipped rather than
sending malformed bytes to the protocol.

### H2 — Centralised Control Risk (multiple changes)

**Removed: pause() / unpause()**
A paused contract prevented `unstake()` and `withdraw_all()`, giving the
pool owner the ability to permanently lock delegator funds. Removed entirely.
No legitimate emergency use case justifies this risk.

**Removed: fix_pool_accounting(), fix_delegator_shares(), fix_delegator_unstaked()**
These were debugging tools from v8 needed to recover from earlier contract bugs.
They gave the owner direct ability to zero out any delegator's balance or corrupt
pool totals. The contract is now correct and no longer needs them.

**Removed: debug_get_stake_shares(), debug_amount_for_shares()**
Unnecessary internal state exposure. All diagnostics available via `get_account()`.

**Claim fee hard-capped at 10% (MAX_CLAIM_FEE_BPS = 1000)**
Previously capped at 100% (10,000 bps). A malicious owner could have changed
the claim fee to 100% after users staked, stealing all rewards. 10% is a
generous upper bound consistent with the NEAR validator ecosystem.

**Fee increase timelock (48 hours)**
Any fee increase is queued via `propose_fee_update()` with a 48-hour delay.
Delegators can observe the pending change and unstake before it applies.
Fee decreases take effect immediately (always user-favorable).
New methods: `propose_fee_update()`, `execute_fee_update()`, `cancel_fee_update()`.

**Added: lock_upgrades(deployer_key)**
Owner can call this once to delete their full-access key from the validator
account, making the contract permanently non-upgradeable. Irreversible.
Sets `upgrades_locked = true` on-chain for delegators to verify.

---

## MEDIUM Severity Fixes

### M2 — parse_key_string Error Handling
Replaced `.expect()` and `panic!()` calls with `require!()` macros.
Produces clean human-readable error messages on invalid key input.

### M3 — Slippage Protection in deposit_and_stake
Added optional `min_shares_out: Option<U128>` parameter.
If provided and non-zero, the transaction reverts if shares received would be
less than the specified minimum. Protects against epoch-boundary share price
spikes between transaction submission and execution.

### M4 — Integer Overflow in Fee Calculations
Fee math now uses `muldiv128(amount, fee_bps, 10_000)` instead of direct
multiplication. Prevents hypothetical u128 overflow for extremely large deposits.

---

## NOT Changed (Audit Finding Disagreed)

### M1 — Division by Zero in muldiv128
The audit flagged potential division-by-zero in the complex path of `muldiv128`.
This finding is incorrect. The `if c == 0 { return 0; }` guard at the top
of the function covers ALL subsequent code paths — Rust's control flow guarantees
c != 0 for every division in the function body. No change was made.

---

## Functions Removed vs Added

| Removed (v11 → v2) | Reason |
|---|---|
| `pause()` / `unpause()` | Fund-locking attack vector |
| `fix_pool_accounting()` | Balance manipulation vector |
| `fix_delegator_shares()` | Direct theft vector |
| `fix_delegator_unstaked()` | Withdrawal manipulation |
| `debug_get_stake_shares()` | Unnecessary |
| `debug_amount_for_shares()` | Unnecessary |
| `update_fees()` | Replaced by timelock version |

| Added (v2) | Purpose |
|---|---|
| `propose_fee_update()` | Timelock-gated fee changes |
| `execute_fee_update()` | Applies fee change after timelock |
| `cancel_fee_update()` | Owner cancels pending fee change |
| `lock_upgrades()` | Permanently removes deployer key |
| `get_pending_fee_update()` | View pending fee change |
| `is_upgrades_locked()` | View upgrade lock status |
| `transfer_ownership()` | Transfer pool ownership |
