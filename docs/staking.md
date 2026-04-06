# Staking

The staking pool contract (`fl_staking_pool v10`) is deployed to each validator account. Delegators deposit FLC and the pool manages staked balances, lockups, unbonding, and reward attribution.

## Parameters

| Parameter | Value |
|---|---|
| Base APY | 20% (halving schedule applies) |
| Epoch length | 43,200 blocks (~12 hours) |
| Lockup period | 48 hours after any deposit or compound |
| Unbonding period | 4 epochs (~48 hours) after unstaking |
| Deposit fee | 0.1% (10 bps), deducted before shares are minted |
| Claim fee | 0.1% (10 bps), deducted from claimed rewards |
| Compound fee | None |
| Minimum stake | 1 FLC |

## Contract functions

### deposit_and_stake()

Deposits FLC and stakes it immediately. Deducts the 0.1% deposit fee before computing shares. Sets `unlock_timestamp_ns` to 48 hours from now.

If this is the first deposit into an empty pool, `last_locked_balance` is synced to the current protocol-locked amount before `internal_ping()` runs — this prevents phantom rewards for the first delegator.

### compound()

Reinvests accrued rewards back into staked principal without withdrawing them. Increases staked balance and resets the 48-hour lock. No fee. More efficient than claim + re-deposit: saves gas and avoids paying the 0.1% deposit fee on the compounded amount.

### claim_rewards()

Transfers accrued rewards (minus the 0.1% claim fee) to the caller's account. Requires the 48-hour lock to have expired. Resets the lock, so avoid calling this if you plan to unstake soon — it will restart the 48-hour window.

### unstake(amount)

Moves a specific amount (in yoctoFLC) from staked to unstaked balance. Requires the 48-hour lock to have expired. Sets `unstake_available_epoch` to current epoch + 4.

### unstake_all()

Moves the entire staked balance to unstaked balance. Requires the 48-hour lock to have expired. A sub-FLC dust residue (< 1 FLC) may remain from double floor-rounding in share math — this is harmless and stays in the pool as a microscopic share-price lift.

### withdraw_all()

Sends the unstaked balance back to the caller's wallet. Requires the 4-epoch unbonding period to have completed (`unstake_available_epoch <= current_epoch`).

### ping()

Triggers `internal_ping()` manually, updating the pool's share price to reflect the latest epoch rewards. Call this if you want to see updated balances without changing your position.

## Internal reward accounting (v10)

`internal_ping()` runs at the start of every state-changing call. It reads `env::account_locked_balance()` and compares to `last_locked_balance` (the locked amount as of the previous call). The difference is the epoch reward delivered by the protocol.

Prior to v10, the entire reward delta was added to `total_staked_balance`. This caused validator self-stake yield to leak to delegators — delegators were credited a share of rewards earned by the validator's own stake.

v10 fixes this with proportional attribution:

```rust
let delegator_reward = muldiv128(
    total_reward,
    self.total_staked_balance,
    self.last_locked_balance,
).min(total_reward);
self.total_staked_balance += delegator_reward;
```

This ensures:
- `delegator_reward = total_reward × (delegator_staked / total_locked)`
- Validators earn on their own stake at the validator APY
- Delegators earn on their contributed stake at the delegator APY
- No cross-contamination between the two

`muldiv128` uses 128-bit intermediate arithmetic to prevent overflow on large balances.

## Lock enforcement

The 48-hour lock check (applied to `unstake`, `unstake_all`, `claim_rewards`):

```rust
require!(
    env::block_timestamp() >= self.unlock_timestamp_ns,
    "Stake is still locked"
);
```

The 4-epoch unbonding check (applied to `withdraw_all`):

```rust
require!(
    account.unstaked_available_epoch_height <= env::epoch_height(),
    "The unstaked balance is not yet available due to unbonding period"
);
```

Both checks are enforced on-chain before any balance moves. Transactions that call too early are rejected.

## Share-price model

The pool tracks balances via shares rather than raw FLC amounts:

```
share_price  = total_staked_balance / total_share_balance
your_balance = your_staked_shares × share_price
```

When rewards arrive, `total_staked_balance` increases but `total_share_balance` stays the same — so the share price rises. All existing holders benefit proportionally without any on-chain action. New depositors receive shares at the current (higher) price, so they cannot retroactively claim rewards earned before they joined.

## PQC key registration

Validators register their block-signing key with `update_staking_key()`. Key lengths are validated on-chain:

| Algorithm | Standard | Public key length |
|---|---|---|
| FN-DSA (Falcon-512) | FIPS 206 | 897 bytes |
| ML-DSA (Dilithium3) | FIPS 204 | 1952 bytes |
| SLH-DSA (SPHINCS+-128) | FIPS 205 | 32 bytes |

Wrong length causes an explicit on-chain panic with a descriptive message. No silent truncation.

## Version history

| Version | Change |
|---|---|
| v1 | Initial contract; wrong key bytes (missing Borsh length prefix) |
| v2 | Fixed key encoding |
| v3 | Added 48-hour lockup mechanism |
| v4 | Added exact-length PQC key validation |
| v5 | Fixed first-delegator phantom reward (bootstrap sync) |
| v6 | Fixed unstake share burn precision (rounding bug) |
| v7 | Fixed withdraw double-debit (unstaked balance decremented twice) |
| v8 | Fixed compound resetting lock to 0 instead of now+48h |
| v9 | Fixed validator stake drop in internal_restake (added max guard) |
| v10 | Proportional reward attribution — prevents self-stake yield leaking to delegators |

Full details: [staking-pool-bug-history.md](staking-pool-bug-history.md)
