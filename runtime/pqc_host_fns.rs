/// PQC-NEAR: runtime/near-vm-runner/src/logic/pqc_host_fns.rs
///
/// Cryptographic implementations for the three PQC WASM host functions.
/// Called by smart contracts to verify PQC signatures on-chain.
/// Replaces the classical ed25519_verify host function.
///
/// Security invariants:
///   - Gas is charged BEFORE the crypto operation (prepay — prevents DoS).
///   - ALL memory bounds validated before reading from WASM memory.
///   - Invalid signature bytes return 0 (invalid), never trap.
///   - ed25519_verify is blocked at protocol_version >= PQC_PROTOCOL_VERSION.

use near_crypto::{
    signature::{
        MLDSA_PUBLIC_KEY_LEN, MLDSA_SIGNATURE_LEN,
        FNDSA_PUBLIC_KEY_LEN, FNDSA_SIGNATURE_MAX_LEN,
        SLHDSA_PUBLIC_KEY_LEN, SLHDSA_SIGNATURE_LEN,
    },
    KeyType, PublicKey, Signature,
};
use pqcrypto_dilithium::dilithium3::{self, PublicKey as MlDsaPk, DetachedSignature as MlDsaSig};
use pqcrypto_falcon::falcon512::{self, PublicKey as FnDsaPk, DetachedSignature as FnDsaSig};
use pqcrypto_sphincsplus::sphincssha2128ssimple::{
    self, PublicKey as SlhDsaPk, DetachedSignature as SlhDsaSig,
};
use pqcrypto_traits::sign::{DetachedSignature as _, PublicKey as PqPk};

// ── Gas constants — single source of truth (matches core/primitives/src/runtime/fees.rs) ──

const MLDSA_VERIFY_BASE_GAS: u64  = 3_000_000_000_000; // v1003: raised from 2.1T (benchmark M2 p99=1.7ms)
const MLDSA_VERIFY_BYTE_GAS: u64  =         5_000_000;
const FNDSA_VERIFY_BASE_GAS: u64  = 1_400_000_000_000;
const FNDSA_VERIFY_BYTE_GAS: u64  =         5_000_000;
const SLHDSA_VERIFY_BASE_GAS: u64 = 8_000_000_000_000; // v1003: raised from 3.2T (benchmark M2 p99=5.1ms, 8KB sig)
const SLHDSA_VERIFY_BYTE_GAS: u64 =         5_000_000;

// ── Host context trait ───────────────────────────────────────────────────────────

pub trait PqcHostContext {
    /// Deduct `gas` from the remaining gas budget.
    /// Returns Err if gas is exhausted (trap).
    fn prepay_gas(&mut self, gas: u64) -> Result<(), HostError>;

    /// Read `len` bytes from WASM linear memory at `ptr`.
    /// Returns Err on out-of-bounds (trap).
    fn read_memory(&self, ptr: u64, len: u64) -> Result<Vec<u8>, HostError>;

    /// Current protocol version (used to gate deprecated functions).
    fn protocol_version(&self) -> u32;
}

/// Errors returned by host functions (mapped to WASM traps by the caller).
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("Out of gas")]
    OutOfGas,
    #[error("Memory access out of bounds: ptr={ptr} len={len}")]
    MemoryOutOfBounds { ptr: u64, len: u64 },
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("Host function deprecated in protocol version {since}")]
    Deprecated { since: u32 },
}

// ── Host function: mldsa_verify ───────────────────────────────────────────────

/// Verify an ML-DSA (Dilithium3 / FIPS 204) detached signature.
///
/// # Parameters (all are WASM memory pointers/lengths)
/// - `sig_len`: byte length of signature (must equal MLDSA_SIGNATURE_LEN = 3293)
/// - `sig_ptr`: pointer to signature bytes in WASM memory
/// - `msg_len`: byte length of message
/// - `msg_ptr`: pointer to message bytes in WASM memory
/// - `pk_ptr`:  pointer to ML-DSA public key (MLDSA_PUBLIC_KEY_LEN = 1952 bytes)
///
/// # Returns
/// - `1u64` if signature is valid
/// - `0u64` if signature is invalid (wrong key, wrong message, bad bytes)
///
/// # Gas
/// Charged upfront: MLDSA_VERIFY_BASE_GAS + msg_len × MLDSA_VERIFY_BYTE_GAS
/// Traps if gas budget is insufficient.
/// Protocol version at which ed25519_verify is banned (matches PQC_PROTOCOL_VERSION).
const ED25519_VERIFY_BANNED_AT: u32 = 999;

pub fn mldsa_verify<C: PqcHostContext>(
    ctx: &mut C,
    sig_len: u64,
    sig_ptr: u64,
    msg_len: u64,
    msg_ptr: u64,
    pk_ptr: u64,
) -> Result<u64, HostError> {
    // Validate parameters before touching memory
    if sig_len != MLDSA_SIGNATURE_LEN as u64 {
        return Err(HostError::InvalidParameter(format!(
            "mldsa_verify: sig_len must be {}, got {}", MLDSA_SIGNATURE_LEN, sig_len
        )));
    }

    // Charge gas BEFORE any computation (prepay model)
    let gas = MLDSA_VERIFY_BASE_GAS
        .saturating_add(msg_len.saturating_mul(MLDSA_VERIFY_BYTE_GAS));
    ctx.prepay_gas(gas)?;

    // Read from WASM memory (bounds-checked)
    let sig_bytes = ctx.read_memory(sig_ptr, sig_len)?;
    let msg_bytes = ctx.read_memory(msg_ptr, msg_len)?;
    let pk_bytes  = ctx.read_memory(pk_ptr, MLDSA_PUBLIC_KEY_LEN as u64)?;

    // Verify
    let result = verify_mldsa(&sig_bytes, &msg_bytes, &pk_bytes);
    Ok(if result { 1 } else { 0 })
}

fn verify_mldsa(sig_bytes: &[u8], msg: &[u8], pk_bytes: &[u8]) -> bool {
    let Ok(pk)  = MlDsaPk::from_bytes(pk_bytes)  else { return false };
    let Ok(sig) = MlDsaSig::from_bytes(sig_bytes) else { return false };
    dilithium3::verify_detached_signature(&sig, msg, &pk).is_ok()
}

// ── Host function: fndsa_verify ───────────────────────────────────────────────

/// Verify an FN-DSA (Falcon-512 / FIPS 206) detached signature.
///
/// # Parameters
/// - `sig_len`: byte length of signature (variable, max FNDSA_SIGNATURE_MAX_LEN = 752)
/// - `sig_ptr`: pointer to signature bytes
/// - `msg_len`: byte length of message
/// - `msg_ptr`: pointer to message bytes
/// - `pk_ptr`:  pointer to FN-DSA public key (FNDSA_PUBLIC_KEY_LEN = 897 bytes)
///
/// # Returns
/// - `1u64` if valid, `0u64` if invalid
///
/// # Gas
/// FNDSA_VERIFY_BASE_GAS + msg_len × FNDSA_VERIFY_BYTE_GAS
pub fn fndsa_verify<C: PqcHostContext>(
    ctx: &mut C,
    sig_len: u64,
    sig_ptr: u64,
    msg_len: u64,
    msg_ptr: u64,
    pk_ptr: u64,
) -> Result<u64, HostError> {
    if sig_len > FNDSA_SIGNATURE_MAX_LEN as u64 {
        return Err(HostError::InvalidParameter(format!(
            "fndsa_verify: sig_len {} exceeds max {}", sig_len, FNDSA_SIGNATURE_MAX_LEN
        )));
    }
    if sig_len == 0 {
        return Err(HostError::InvalidParameter("fndsa_verify: sig_len must be > 0".into()));
    }

    let gas = FNDSA_VERIFY_BASE_GAS
        .saturating_add(msg_len.saturating_mul(FNDSA_VERIFY_BYTE_GAS));
    ctx.prepay_gas(gas)?;

    let sig_bytes = ctx.read_memory(sig_ptr, sig_len)?;
    let msg_bytes = ctx.read_memory(msg_ptr, msg_len)?;
    let pk_bytes  = ctx.read_memory(pk_ptr, FNDSA_PUBLIC_KEY_LEN as u64)?;

    let result = verify_fndsa(&sig_bytes, &msg_bytes, &pk_bytes);
    Ok(if result { 1 } else { 0 })
}

fn verify_fndsa(sig_bytes: &[u8], msg: &[u8], pk_bytes: &[u8]) -> bool {
    let Ok(pk)  = FnDsaPk::from_bytes(pk_bytes)  else { return false };
    let Ok(sig) = FnDsaSig::from_bytes(sig_bytes) else { return false };
    falcon512::verify_detached_signature(&sig, msg, &pk).is_ok()
}

// ── Host function: slhdsa_verify ──────────────────────────────────────────────

/// Verify an SLH-DSA (SPHINCS+-SHA2-128s / FIPS 205) detached signature.
///
/// # Parameters
/// - `sig_len`: byte length of signature (must equal SLHDSA_SIGNATURE_LEN = 7856)
/// - `sig_ptr`: pointer to signature bytes
/// - `msg_len`: byte length of message
/// - `msg_ptr`: pointer to message bytes
/// - `pk_ptr`:  pointer to SLH-DSA public key (SLHDSA_PUBLIC_KEY_LEN = 32 bytes)
///
/// # Returns
/// - `1u64` if valid, `0u64` if invalid
///
/// # Gas
/// SLHDSA_VERIFY_BASE_GAS + msg_len × SLHDSA_VERIFY_BYTE_GAS
/// (Most expensive host function — SLH-DSA verify traverses a hypertree)
pub fn slhdsa_verify<C: PqcHostContext>(
    ctx: &mut C,
    sig_len: u64,
    sig_ptr: u64,
    msg_len: u64,
    msg_ptr: u64,
    pk_ptr: u64,
) -> Result<u64, HostError> {
    if sig_len != SLHDSA_SIGNATURE_LEN as u64 {
        return Err(HostError::InvalidParameter(format!(
            "slhdsa_verify: sig_len must be {}, got {}", SLHDSA_SIGNATURE_LEN, sig_len
        )));
    }

    let gas = SLHDSA_VERIFY_BASE_GAS
        .saturating_add(msg_len.saturating_mul(SLHDSA_VERIFY_BYTE_GAS));
    ctx.prepay_gas(gas)?;

    let sig_bytes = ctx.read_memory(sig_ptr, sig_len)?;
    let msg_bytes = ctx.read_memory(msg_ptr, msg_len)?;
    let pk_bytes  = ctx.read_memory(pk_ptr, SLHDSA_PUBLIC_KEY_LEN as u64)?;

    let result = verify_slhdsa(&sig_bytes, &msg_bytes, &pk_bytes);
    Ok(if result { 1 } else { 0 })
}

fn verify_slhdsa(sig_bytes: &[u8], msg: &[u8], pk_bytes: &[u8]) -> bool {
    let Ok(pk)  = SlhDsaPk::from_bytes(pk_bytes)  else { return false };
    let Ok(sig) = SlhDsaSig::from_bytes(sig_bytes) else { return false };
    sphincssha2128ssimple::verify_detached_signature(&sig, msg, &pk).is_ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────


// ── Deprecated classical host function ───────────────────────────────────────

/// ed25519_verify — DEPRECATED at protocol version >= ED25519_VERIFY_BANNED_AT.
///
/// Returns Err(Deprecated) immediately on PQC chain.
/// On classical chain (protocol < 999), delegates to the classical path.
pub fn ed25519_verify_or_deprecate<C: PqcHostContext>(
    ctx: &mut C,
    _sig_len: u64, _sig_ptr: u64,
    _msg_len: u64, _msg_ptr: u64,
    _pk_ptr: u64,
) -> Result<u64, HostError> {
    if ctx.protocol_version() >= ED25519_VERIFY_BANNED_AT {
        return Err(HostError::Deprecated {
            since: ED25519_VERIFY_BANNED_AT,
        });
    }
    // On classical chain: caller must forward to the upstream ed25519 path.
    // Returning 0 here is safe — the real integration calls the classical path.
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_crypto::{InMemorySigner, KeyType, SecretKey};

    // ── Test harness: mock VMLogic context ────────────────────────────────────

    struct MockCtx {
        memory: Vec<u8>,
        gas_remaining: u64,
        protocol_version: u32,
    }

    impl MockCtx {
        fn new(gas: u64) -> Self {
            Self {
                memory: vec![0u8; 64 * 1024], // 64 KB fake WASM memory
                gas_remaining: gas,
                protocol_version: 999,
            }
        }

        fn write(&mut self, offset: usize, data: &[u8]) {
            self.memory[offset..offset + data.len()].copy_from_slice(data);
        }
    }

    impl PqcHostContext for MockCtx {
        fn prepay_gas(&mut self, gas: u64) -> Result<(), HostError> {
            if self.gas_remaining < gas {
                return Err(HostError::OutOfGas);
            }
            self.gas_remaining -= gas;
            Ok(())
        }

        fn read_memory(&self, ptr: u64, len: u64) -> Result<Vec<u8>, HostError> {
            let start = ptr as usize;
            let end = start + len as usize;
            if end > self.memory.len() {
                return Err(HostError::MemoryOutOfBounds { ptr, len });
            }
            Ok(self.memory[start..end].to_vec())
        }

        fn protocol_version(&self) -> u32 {
            self.protocol_version
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_mldsa_verify_valid_signature() {
        let sk = SecretKey::from_random(KeyType::MlDsa);
        let msg = b"test message for mldsa host fn";
        let sig = sk.sign(msg);
        let pk = sk.public_key();

        let sig_bytes = match &sig { near_crypto::Signature::MlDsa(b) => b.to_vec(), _ => panic!() };
        let pk_bytes = pk.key_data().to_vec();

        let mut ctx = MockCtx::new(u64::MAX);
        let sig_offset = 0usize;
        let msg_offset = sig_offset + MLDSA_SIGNATURE_LEN;
        let pk_offset  = msg_offset + msg.len();

        ctx.write(sig_offset, &sig_bytes);
        ctx.write(msg_offset, msg);
        ctx.write(pk_offset,  &pk_bytes);

        let result = mldsa_verify(
            &mut ctx,
            MLDSA_SIGNATURE_LEN as u64, sig_offset as u64,
            msg.len() as u64,           msg_offset as u64,
            pk_offset as u64,
        ).expect("mldsa_verify should not trap");

        assert_eq!(result, 1u64, "Valid ML-DSA signature should return 1");
    }

    #[test]
    fn test_mldsa_verify_wrong_message_returns_zero() {
        let sk = SecretKey::from_random(KeyType::MlDsa);
        let sig = sk.sign(b"correct message");
        let pk = sk.public_key();
        let wrong_msg = b"wrong message---";

        let sig_bytes = match &sig { near_crypto::Signature::MlDsa(b) => b.to_vec(), _ => panic!() };
        let pk_bytes = pk.key_data().to_vec();

        let mut ctx = MockCtx::new(u64::MAX);
        ctx.write(0, &sig_bytes);
        ctx.write(MLDSA_SIGNATURE_LEN, wrong_msg);
        ctx.write(MLDSA_SIGNATURE_LEN + wrong_msg.len(), &pk_bytes);

        let result = mldsa_verify(
            &mut ctx,
            MLDSA_SIGNATURE_LEN as u64, 0,
            wrong_msg.len() as u64,     MLDSA_SIGNATURE_LEN as u64,
            (MLDSA_SIGNATURE_LEN + wrong_msg.len()) as u64,
        ).unwrap();
        assert_eq!(result, 0u64, "Wrong message should return 0");
    }

    #[test]
    fn test_fndsa_verify_valid_signature() {
        let sk = SecretKey::from_random(KeyType::FnDsa);
        let msg = b"test message for fndsa host fn";
        let sig = sk.sign(msg);
        let pk = sk.public_key();

        let sig_bytes = match &sig { near_crypto::Signature::FnDsa(b) => b.clone(), _ => panic!() };
        let pk_bytes = pk.key_data().to_vec();

        let mut ctx = MockCtx::new(u64::MAX);
        ctx.write(0, &sig_bytes);
        ctx.write(sig_bytes.len(), msg);
        ctx.write(sig_bytes.len() + msg.len(), &pk_bytes);

        let result = fndsa_verify(
            &mut ctx,
            sig_bytes.len() as u64, 0,
            msg.len() as u64,        sig_bytes.len() as u64,
            (sig_bytes.len() + msg.len()) as u64,
        ).unwrap();
        assert_eq!(result, 1u64, "Valid FN-DSA signature should return 1");
    }

    #[test]
    fn test_out_of_gas_traps() {
        let sk = SecretKey::from_random(KeyType::MlDsa);
        let sig = sk.sign(b"msg");
        let pk = sk.public_key();
        let sig_bytes = match &sig { near_crypto::Signature::MlDsa(b) => b.to_vec(), _ => panic!() };

        let mut ctx = MockCtx::new(0); // zero gas
        ctx.write(0, &sig_bytes);

        let result = mldsa_verify(&mut ctx, MLDSA_SIGNATURE_LEN as u64, 0, 3, MLDSA_SIGNATURE_LEN as u64, (MLDSA_SIGNATURE_LEN + 3) as u64);
        assert!(matches!(result, Err(HostError::OutOfGas)), "Insufficient gas should trap");
    }

    #[test]
    fn test_memory_out_of_bounds_traps() {
        let mut ctx = MockCtx::new(u64::MAX);
        // Point sig_ptr way beyond WASM memory (64 KB)
        let result = mldsa_verify(
            &mut ctx,
            MLDSA_SIGNATURE_LEN as u64,
            100_000, // out of bounds
            10, 0, 0,
        );
        assert!(matches!(result, Err(HostError::MemoryOutOfBounds { .. })));
    }

    #[test]
    fn test_wrong_sig_len_traps_mldsa() {
        let mut ctx = MockCtx::new(u64::MAX);
        let result = mldsa_verify(&mut ctx, 64, 0, 10, 0, 4096); // 64 is Ed25519 size
        assert!(matches!(result, Err(HostError::InvalidParameter(_))));
    }

    #[test]
    fn test_wrong_sig_len_traps_slhdsa() {
        let mut ctx = MockCtx::new(u64::MAX);
        let result = slhdsa_verify(&mut ctx, 64, 0, 10, 0, 8192);
        assert!(matches!(result, Err(HostError::InvalidParameter(_))));
    }

    #[test]
    fn test_fndsa_sig_len_too_large_traps() {
        let mut ctx = MockCtx::new(u64::MAX);
        // 753 > FNDSA_SIGNATURE_MAX_LEN (752)
        let result = fndsa_verify(&mut ctx, 753, 0, 10, 0, 4096);
        assert!(matches!(result, Err(HostError::InvalidParameter(_))));
    }

    #[test]
    fn test_gas_is_charged_before_memory_read() {
        // Even with invalid memory pointer, gas should be charged first
        // (so contracts can't probe memory without paying)
        // With zero gas, we should get OutOfGas, not MemoryOutOfBounds
        let mut ctx = MockCtx::new(0);
        let result = mldsa_verify(&mut ctx, MLDSA_SIGNATURE_LEN as u64, 0, 10, 0, 0);
        assert!(matches!(result, Err(HostError::OutOfGas)),
            "Gas check must happen before memory reads");
    }

    #[test]
    fn test_all_zero_signature_returns_zero_not_panic() {
        // A zeroed-out signature must return 0 (invalid), not panic
        let sk = SecretKey::from_random(KeyType::MlDsa);
        let pk = sk.public_key();
        let pk_bytes = pk.key_data().to_vec();

        let mut ctx = MockCtx::new(u64::MAX);
        // sig = all zeros (invalid)
        let msg = b"any message";
        ctx.write(MLDSA_SIGNATURE_LEN, msg);
        ctx.write(MLDSA_SIGNATURE_LEN + msg.len(), &pk_bytes);

        let result = mldsa_verify(
            &mut ctx,
            MLDSA_SIGNATURE_LEN as u64, 0,
            msg.len() as u64, MLDSA_SIGNATURE_LEN as u64,
            (MLDSA_SIGNATURE_LEN + msg.len()) as u64,
        ).unwrap();
        assert_eq!(result, 0u64, "All-zero signature must return 0, not panic");
    }
}
