# Build stage
FROM rust:bookworm AS builder
 
WORKDIR /app
COPY . .
# Install build dependencies
RUN apt-get update && apt-get install -y libclang-dev libssl-dev
# Build only the cardano-utxo-monitor package
RUN cargo build --release --package cardano-utxo-monitor
 
# Final run stage
FROM debian:bookworm-slim AS runner
 
WORKDIR /app
# Copy the compiled binary from the builder stage
COPY --from=builder /app/target/release/cardano-utxo-monitor /app/cardano-utxo-monitor

# The service only needs the binary to run, no build tools required.
ENTRYPOINT ["/app/cardano-utxo-monitor"]