# Gas Rationale

## The problem with the v1002 constants

The initial gas constants in v1001/v1002 were estimates based on key size ratios, not measured verification times. That turned out to be wrong — verification time doesn't scale linearly with key or signature size. SLH-DSA in particular was severely underpriced: at 3.2 TGas it would let an attacker submit transactions that take longer to verify than a block time on a min-spec validator.

## Benchmark methodology

1000 iterations of each algorithm on two real validator machines. Each iteration: generate key pair, sign a 64-byte message, verify. Wall-clock nanoseconds recorded per verification.

Gas constants are calibrated to the slower machine at p99. If we calibrated to the faster machine, validators with less powerful hardware would struggle on adversarial blocks.

Machine A: 4-core Intel Skylake, 16GB RAM.
Machine B: 2-core Intel Xeon Skylake, 4GB RAM. This is the min-spec reference.

## Results

Machine A:

| Algorithm | p50 | p95 | p99 |
|---|---|---|---|
| FN-DSA | 0.041ms | 0.148ms | 0.192ms |
| ML-DSA | 0.162ms | 0.521ms | 0.894ms |
| SLH-DSA | 0.626ms | 1.847ms | 2.203ms |

Machine B (min-spec):

| Algorithm | p50 | p95 | p99 |
|---|---|---|---|
| FN-DSA | 0.055ms | 0.187ms | 0.241ms |
| ML-DSA | 0.330ms | 1.124ms | 1.703ms |
| SLH-DSA | 0.972ms | 3.614ms | 5.098ms |

## How the constants were set

NEAR's model is roughly 1 TGas per millisecond of compute. Using min-spec p99:

FN-DSA at 0.241ms gets 1.4 TGas — about 5.8x headroom. It's the default algorithm for most users so we kept it cheap deliberately. The large multiplier also means it won't need another raise as hardware varies.

ML-DSA at 1.703ms gets 3.0 TGas — 1.76x headroom. Raised from 2.1 TGas in v1003.

SLH-DSA at 5.098ms gets 8.0 TGas — 1.57x headroom. Raised from 3.2 TGas in v1003. The raise is large for two reasons: verification is the slowest of the three schemes, and signatures are ~8KB which also burns chunk space. At 3.2 TGas the DoS math didn't work out in favor of validators.

## Protocol history

v1001: FN-DSA 1.4 TGas, ML-DSA 2.1 TGas, SLH-DSA 3.2 TGas (initial estimates).
v1003: ML-DSA raised to 3.0 TGas, SLH-DSA raised to 8.0 TGas based on benchmark data.

Gas changes are consensus-critical and require a hard fork because all validators must agree on the cost of every operation.
