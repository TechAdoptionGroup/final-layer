FROM ubuntu:22.04 AS build

RUN apt-get update -qq && apt-get install -y     curl     pkg-config     libssl-dev     && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup     CARGO_HOME=/usr/local/cargo     PATH=/usr/local/cargo/bin:$PATH

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --no-modify-path --default-toolchain 1.86.0

RUN rustup target add wasm32-unknown-unknown

WORKDIR /app
COPY . .

RUN cargo build -p staking-pool --target wasm32-unknown-unknown --release

FROM ubuntu:22.04

COPY --from=build     /app/target/wasm32-unknown-unknown/release/staking_pool.wasm     /output/staking_pool.wasm

CMD ["sh", "-c", "echo 'Build complete. WASM at /output/staking_pool.wasm'"]
