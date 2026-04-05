/*!
 * Final Layer Staking Pool Contract (near-sdk 5.x, legacy collections)
 *
 * Allows users to delegate FLC to a validator and earn staking rewards.
 * Features:
 *   - Deposit fee (max 0.1% = 10 bps) charged on each stake
 *   - Claim fee (validator's choice, in bps) taken from reward withdrawals
 *   - 48-hour lockup (4 epochs × 43 200 blocks × 1 s/block = 172 800 s)
 *     Any action (deposit, unstake, claim, compound) resets the lockup timer
 *   - 4-epoch unbonding after unstake before funds can be withdrawn
 *   - Share-based accounting: rewards accrue as the share price rises
 */

use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::collections::LookupMap;
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, require, AccountId, NearToken};

// ── Constants ─────────────────────────────────────────────────────────────────

const LOCKUP_NS: u64        = 4 * 43_200 * 1_000_000_000;
const NUM_EPOCHS_TO_UNLOCK: u64 = 4;
const MAX_DEPOSIT_FEE_BPS: u16  = 10;
const MIN_STAKE: u128           = 1_000_000_000_000_000_000_000_000; // 1 FLC

// ── Data types ────────────────────────────────────────────────────────────────

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, Default)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub struct Delegator {
    pub stake_shares:           u128,
    pub principal:              u128,
    pub unstaked_balance:       u128,
    pub unstake_available_epoch: u64,
    pub unlock_timestamp_ns:    u64,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AccountView {
    pub staked_balance:          U128,
    pub unstaked_balance:        U128,
    pub total_balance:           U128,
    pub principal:               U128,
    pub rewards_earned:          U128,
    pub can_withdraw:            bool,
    pub is_locked:               bool,
    pub unlock_timestamp_ns:     u64,
    pub unstake_available_epoch: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PoolFees {
    pub deposit_fee_bps: u16,
    pub claim_fee_bps:   u16,
}

// ── Raw WASM host calls for PQC staking key support ───────────────────────────

#[cfg(target_arch = "wasm32")]
unsafe fn sys_promise_batch_create(account_id: &str) -> u64 {
    extern "C" {
        fn promise_batch_create(account_id_len: u64, account_id_ptr: u64) -> u64;
    }
    promise_batch_create(account_id.len() as u64, account_id.as_ptr() as u64)
}

#[cfg(target_arch = "wasm32")]
unsafe fn sys_promise_batch_action_stake(promise_idx: u64, amount: u128, pk_bytes: &[u8]) {
    // near-sdk's compiled rlib defines promise_batch_action_stake with 4 params:
    // amount is always a 16-byte LE u128, so amount_len is implicit (no length param).
    extern "C" {
        fn promise_batch_action_stake(
            promise_index:  u64,
            amount_ptr:     u64, // pointer to 16-byte LE u128
            public_key_len: u64,
            public_key_ptr: u64,
        );
    }
    let le = amount.to_le_bytes();
    promise_batch_action_stake(
        promise_idx,
        le.as_ptr() as u64,
        pk_bytes.len() as u64,
        pk_bytes.as_ptr() as u64,
    );
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[near(contract_state)]
pub struct StakingPool {
    pub owner_id:             AccountId,
    pub staking_key_bytes:    Vec<u8>,
    pub deposit_fee_bps:      u16,
    pub claim_fee_bps:        u16,
    pub total_staked_balance: u128,
    pub total_stake_shares:   u128,
    pub last_locked_balance:  u128,
    pub delegators:           LookupMap<AccountId, Delegator>,
    pub paused:               bool,
}

impl Default for StakingPool {
    fn default() -> Self {
        panic!("StakingPool must be initialized via new()")
    }
}

#[near]
impl StakingPool {

    #[init]
    pub fn new(
        owner_id: AccountId,
        staking_key: String,
        deposit_fee_bps: u16,
        claim_fee_bps: u16,
    ) -> Self {
        require!(deposit_fee_bps <= MAX_DEPOSIT_FEE_BPS, "deposit_fee_bps exceeds 0.1% max");
        require!(claim_fee_bps <= 10_000, "claim_fee_bps > 100%");
        Self {
            owner_id,
            staking_key_bytes: parse_key_string(&staking_key),
            deposit_fee_bps,
            claim_fee_bps,
            total_staked_balance: 0,
            total_stake_shares: 0,
            last_locked_balance: 0,
            delegators: LookupMap::new(b"d".to_vec()),
            paused: false,
        }
    }

    // ── User actions ────────────────────────────────────────────────────────

    #[payable]
    pub fn deposit_and_stake(&mut self) {
        require!(!self.paused, "Pool is paused");
        // Fix: Sync last_locked_balance before creating first shares.
        // Without this, the first delegator captures the validator's entire
        // pre-existing protocol-locked balance as "rewards" via internal_ping().
        if self.total_stake_shares == 0 {
            self.last_locked_balance = env::account_locked_balance().as_yoctonear();
        }
        self.internal_ping();

        let amount = env::attached_deposit().as_yoctonear();
        require!(amount >= MIN_STAKE, "Deposit must be >= 1 FLC");

        let fee = amount * self.deposit_fee_bps as u128 / 10_000;
        let net = amount - fee;
        if fee > 0 {
            near_sdk::Promise::new(self.owner_id.clone())
                .transfer(NearToken::from_yoctonear(fee))
                .detach();
        }

        let shares = self.shares_for(net);
        let account_id = env::predecessor_account_id();
        let mut d = self.delegators.get(&account_id).unwrap_or_default();
        d.stake_shares += shares;
        d.principal += net;
        d.unlock_timestamp_ns = env::block_timestamp() + LOCKUP_NS;
        self.delegators.insert(&account_id, &d);

        self.total_staked_balance += net;
        self.total_stake_shares += shares;
        self.internal_restake();
    }

    pub fn unstake(&mut self, amount: U128) {
        self.internal_ping();
        let account_id = env::predecessor_account_id();
        let mut d = self.delegators.get(&account_id).expect("No stake found");

        require!(
            env::block_timestamp() >= d.unlock_timestamp_ns,
            "Stake is still locked"
        );

        let amt = amount.0;
        let staked = self.amount_for_shares(d.stake_shares);
        require!(amt > 0 && amt <= staked, "Invalid unstake amount");

        let burn = self.shares_for_amount(amt);
        d.stake_shares = d.stake_shares.saturating_sub(burn);
        d.principal    = d.principal.saturating_sub(amt);
        d.unstaked_balance += amt;
        d.unstake_available_epoch = env::epoch_height() + NUM_EPOCHS_TO_UNLOCK;
        d.unlock_timestamp_ns = env::block_timestamp() + LOCKUP_NS;
        self.delegators.insert(&account_id, &d);

        self.total_staked_balance = self.total_staked_balance.saturating_sub(amt);
        self.total_stake_shares   = self.total_stake_shares.saturating_sub(burn);
        self.internal_restake();
    }

    pub fn unstake_all(&mut self) {
        let account_id = env::predecessor_account_id();
        let d = self.delegators.get(&account_id).expect("No stake found");
        let staked = self.amount_for_shares(d.stake_shares);
        require!(staked > 0, "Nothing to unstake");
        self.unstake(U128(staked));
    }

    pub fn withdraw_all(&mut self) {
        let account_id = env::predecessor_account_id();
        let mut d = self.delegators.get(&account_id).expect("No withdrawal found");
        require!(d.unstaked_balance > 0, "No unstaked balance");
        require!(
            env::epoch_height() >= d.unstake_available_epoch,
            "Still in unbonding period"
        );
        let amount = d.unstaked_balance;
        d.unstaked_balance = 0;
        d.unstake_available_epoch = 0;
        self.delegators.insert(&account_id, &d);
        near_sdk::Promise::new(account_id).transfer(NearToken::from_yoctonear(amount)).detach();
    }

    pub fn claim_rewards(&mut self) {
        self.internal_ping();
        let account_id = env::predecessor_account_id();
        let mut d = self.delegators.get(&account_id).expect("No stake found");

        let staked   = self.amount_for_shares(d.stake_shares);
        let rewards  = staked.saturating_sub(d.principal);
        require!(rewards > 0, "No rewards yet");

        let fee        = rewards * self.claim_fee_bps as u128 / 10_000;
        let net_reward = rewards - fee;
        let burn       = self.shares_for_amount(rewards);

        d.stake_shares = d.stake_shares.saturating_sub(burn);
        d.unstaked_balance += net_reward;
        d.unstake_available_epoch = env::epoch_height() + NUM_EPOCHS_TO_UNLOCK;
        d.unlock_timestamp_ns = env::block_timestamp() + LOCKUP_NS;
        self.delegators.insert(&account_id, &d);

        if fee > 0 {
            let fee_shares = self.shares_for_amount_post_reduce(fee, rewards, burn);
            let owner_id = self.owner_id.clone();
            let mut od = self.delegators.get(&owner_id).unwrap_or_default();
            od.stake_shares += fee_shares;
            od.principal += fee;
            self.delegators.insert(&owner_id, &od);
            self.total_stake_shares = self.total_stake_shares
                .saturating_sub(burn)
                .saturating_add(fee_shares);
            self.total_staked_balance = self.total_staked_balance.saturating_sub(net_reward);
        } else {
            self.total_stake_shares   = self.total_stake_shares.saturating_sub(burn);
            self.total_staked_balance = self.total_staked_balance.saturating_sub(rewards);
        }
        self.internal_restake();
    }

    pub fn compound(&mut self) {
        self.internal_ping();
        let account_id = env::predecessor_account_id();
        let mut d = self.delegators.get(&account_id).expect("No stake found");

        let staked  = self.amount_for_shares(d.stake_shares);
        let rewards = staked.saturating_sub(d.principal);
        require!(rewards > 0, "No rewards to compound");

        d.principal = staked;
        d.unlock_timestamp_ns = env::block_timestamp() + LOCKUP_NS;
        self.delegators.insert(&account_id, &d);
    }

    // ── Owner actions ───────────────────────────────────────────────────────

    pub fn update_fees(&mut self, deposit_fee_bps: u16, claim_fee_bps: u16) {
        require!(env::predecessor_account_id() == self.owner_id, "Owner only");
        require!(deposit_fee_bps <= MAX_DEPOSIT_FEE_BPS, "deposit > 0.1% max");
        require!(claim_fee_bps <= 10_000, "claim > 100%");
        self.deposit_fee_bps = deposit_fee_bps;
        self.claim_fee_bps   = claim_fee_bps;
    }

    pub fn update_staking_key(&mut self, new_staking_key: String) {
        require!(env::predecessor_account_id() == self.owner_id, "Owner only");
        self.staking_key_bytes = parse_key_string(&new_staking_key);
        self.internal_restake();
    }

    pub fn pause(&mut self) {
        require!(env::predecessor_account_id() == self.owner_id, "Owner only");
        self.paused = true;
    }

    pub fn unpause(&mut self) {
        require!(env::predecessor_account_id() == self.owner_id, "Owner only");
        self.paused = false;
    }

    // ── View methods ────────────────────────────────────────────────────────

    pub fn get_account(&self, account_id: AccountId) -> AccountView {
        let d = self.delegators.get(&account_id).unwrap_or_default();
        let staked  = self.amount_for_shares(d.stake_shares);
        let rewards = staked.saturating_sub(d.principal);
        AccountView {
            staked_balance:          U128(staked),
            unstaked_balance:        U128(d.unstaked_balance),
            total_balance:           U128(staked + d.unstaked_balance),
            principal:               U128(d.principal),
            rewards_earned:          U128(rewards),
            can_withdraw:            d.unstaked_balance > 0
                                     && env::epoch_height() >= d.unstake_available_epoch,
            is_locked:               env::block_timestamp() < d.unlock_timestamp_ns,
            unlock_timestamp_ns:     d.unlock_timestamp_ns,
            unstake_available_epoch: d.unstake_available_epoch,
        }
    }

    pub fn get_total_staked_balance(&self) -> U128 { U128(self.total_staked_balance) }
    pub fn get_total_stake_shares(&self)   -> U128 { U128(self.total_stake_shares) }
    pub fn get_owner_id(&self)             -> AccountId { self.owner_id.clone() }

    pub fn get_fees(&self) -> PoolFees {
        PoolFees { deposit_fee_bps: self.deposit_fee_bps, claim_fee_bps: self.claim_fee_bps }
    }

    pub fn get_account_staked_balance(&self, account_id: AccountId) -> U128 {
        let d = self.delegators.get(&account_id).unwrap_or_default();
        U128(self.amount_for_shares(d.stake_shares))
    }

    pub fn get_account_unstaked_balance(&self, account_id: AccountId) -> U128 {
        U128(self.delegators.get(&account_id).map(|d| d.unstaked_balance).unwrap_or(0))
    }

    pub fn get_account_total_balance(&self, account_id: AccountId) -> U128 {
        let d = self.delegators.get(&account_id).unwrap_or_default();
        U128(self.amount_for_shares(d.stake_shares) + d.unstaked_balance)
    }

    // ── Internal ────────────────────────────────────────────────────────────

    fn internal_ping(&mut self) {
        let locked = env::account_locked_balance().as_yoctonear();
        if locked > self.last_locked_balance {
            // Rewards or new stake detected: credit difference to pool
            self.total_staked_balance += locked - self.last_locked_balance;
            self.last_locked_balance = locked;
        } else if locked < self.last_locked_balance {
            // Slash or validator kickout: reduce pool balance proportionally
            // (no slashing on Final Layer currently, but handles future cases)
            let decrease = self.last_locked_balance - locked;
            self.total_staked_balance = self.total_staked_balance.saturating_sub(decrease);
            self.last_locked_balance = locked;
        }
    }

    fn internal_restake(&mut self) {
        // Uses the corrected key_type_byte values matching near_crypto::KeyType:
        // mldsa=2, fndsa=3, slhdsa=4 (fixed from wrong 0,1,2 in original deploy).
        if self.staking_key_bytes.is_empty() { return; }
        #[cfg(target_arch = "wasm32")]
        {
            let acct = env::current_account_id().to_string();
            let amt  = self.total_staked_balance;
            let pk   = self.staking_key_bytes.clone();
            unsafe {
                let idx = sys_promise_batch_create(&acct);
                sys_promise_batch_action_stake(idx, amt, &pk);
            }
            self.last_locked_balance = amt;
        }
    }

    fn shares_for(&self, amount: u128) -> u128 {
        if self.total_stake_shares == 0 || self.total_staked_balance == 0 { return amount; }
        self.shares_for_amount(amount)
    }

    fn shares_for_amount(&self, amount: u128) -> u128 {
        if self.total_staked_balance == 0 { return amount; }
        amount.checked_mul(self.total_stake_shares)
            .map(|n| n / self.total_staked_balance)
            .unwrap_or_else(|| (amount / self.total_staked_balance) * self.total_stake_shares)
    }

    fn shares_for_amount_post_reduce(&self, fee: u128, rewards: u128, burned: u128) -> u128 {
        let ps = self.total_staked_balance.saturating_sub(rewards);
        let ph = self.total_stake_shares.saturating_sub(burned);
        if ps == 0 { return fee; }
        fee.checked_mul(ph).map(|n| n / ps)
            .unwrap_or_else(|| (fee / ps) * ph)
    }

    fn amount_for_shares(&self, shares: u128) -> u128 {
        if self.total_stake_shares == 0 { return 0; }
        shares.checked_mul(self.total_staked_balance)
            .map(|n| n / self.total_stake_shares)
            .unwrap_or_else(|| (shares / self.total_stake_shares) * self.total_staked_balance)
    }
}

// ── Key parsing ───────────────────────────────────────────────────────────────

fn parse_key_string(key_str: &str) -> Vec<u8> {
    let colon = key_str.find(':').expect("Key format: 'algo:base58'");
    let algo  = &key_str[..colon];
    let b58   = &key_str[colon + 1..];

    // near_crypto KeyType byte values (must match near_crypto::KeyType discriminants):
    // 0=ED25519, 1=SECP256K1, 2=MLDSA, 3=FNDSA, 4=SLHDSA
    let key_type_byte: u8 = match algo {
        "mldsa"  => 2,
        "fndsa"  => 3,
        "slhdsa" => 4,
        other    => panic!("Unknown key algorithm: {}", other),
    };

    let mut key_bytes = bs58::decode(b58).into_vec().expect("Invalid base58");

    let pk_len: usize = match algo {
        "mldsa"  => 1952,
        "fndsa"  => 897,
        "slhdsa" => 32,
        _        => key_bytes.len(),
    };
    // Reject keys with wrong length (prevents silent truncation of malformed keys)
    if key_bytes.len() != pk_len {
        panic!("Key must be exactly {} bytes for {}, got {}", pk_len, algo, key_bytes.len());
    }

    // Borsh encoding of near_crypto::PublicKey:
    //   Ed25519/Secp256k1: key_type(1) + fixed_array_bytes
    //   MLDSA/FNDSA/SLHDSA: key_type(1) + vec_len_u32_LE(4) + bytes
    // PQC keys use Vec<u8> internally so need the 4-byte Borsh length prefix.
    let mut result = Vec::new();
    result.push(key_type_byte);
    match algo {
        "mldsa" | "fndsa" | "slhdsa" => {
            let len_bytes = (key_bytes.len() as u32).to_le_bytes();
            result.extend_from_slice(&len_bytes);
        }
        _ => {}
    }
    result.extend_from_slice(&key_bytes);
    result
}
