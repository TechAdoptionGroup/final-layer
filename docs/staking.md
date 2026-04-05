# Staking

The staking pool contract (`fl_staking_pool v5`) is deployed to each validator account. Delegators deposit FLC and the pool manages staked balances, lockups, and unbonding.

## Parameters

Lockup period: 48 hours (`LOCKUP_NS = 4 * 43200 * 1_000_000_000` nanoseconds).
Unbonding period: 4 epochs after unstaking (roughly another 48 hours).
Epoch length: 43,200 blocks.

## Lifecycle

**deposit_and_stake()** — Deposits FLC and stakes it immediately. Sets `unlock_timestamp_ns` to 48 hours from now. If this is the first deposit into an empty pool, `last_locked_balance` is synced to the current protocol-locked amount before `internal_ping()` runs, preventing phantom rewards for the first delegator.

**claim_rewards()** — Moves accrued rewards into your unstaked balance. Calling this resets the 48-hour lock, so don't call it if you're planning to unstake soon.

**unstake_all()** — Moves your stake to unstaked balance and starts the unbonding clock. Requires the 48-hour lock to have expired. Sets `unstake_available_epoch` to current epoch + 4.

**withdraw_all()** — Sends unstaked balance back to your wallet. Requires the 4-epoch unbonding period to have completed.

## Enforcement

The lock check:
```rust
require!(
    env::block_timestamp() >= self.unlock_timestamp_ns,
    "Stake is still locked"
);
```

The unbonding check:
```rust
require!(
    account.unstaked_available_epoch_height <= env::epoch_height(),
    "The unstaked balance is not yet available due to unbonding period"
);
```

Both checks happen in the contract before any balance moves. Transactions that call too early are rejected on-chain.

## PQC key registration

Keys registered with the staking pool go through `parse_key_string()` which enforces exact byte lengths: 897 for FN-DSA, 1952 for ML-DSA, 32 for SLH-DSA. Wrong length panics with a descriptive message. Unknown algorithms also panic. No silent truncation.

## Version history

v1 introduced the contract but had wrong key bytes (missing Borsh length prefix). v2 fixed the encoding. v3 added the lockup mechanism. v4 added exact-length key validation. v5 fixed the first-delegator phantom reward bug.
