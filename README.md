# Cardano UTXO Monitor

A mempool-aware UTXO indexing service which provides real-time visibility into spendable UTXOs on Cardano.

This service tracks both confirmed blockchain state and pending mempool transactions to give you the complete picture of which UTXOs are actually available to spend. 

When "Unspent" UTXOs are queried it returns:

- Confirmed UTXOs that aren't being spent by any mempool transaction
- New UTXOs created by transactions currently in the mempool
- Confirmed UTXOs

Note that this requires a connection to a Cardano node's IPC socket.

## Usage

```bash
cargo run --release --package cardano-utxo-monitor -- \
    --config-path /resources/conf.json \
    --log4rs-path /resources/log4rs.yaml \
    --host 0.0.0.0 \
    --port 8080
```

### Docker

Build the image:
```bash
docker build -t cardano-utxo-monitor:latest .
```

Run the container:
```bash
docker run -d \
  --name utxo-monitor \
  -p 8080:8080 \
  -v /path/to/your/cardano-node/ipc:/ipc \
  -v /path/to/your/utxo-monitor-config:/config \
  -v /path/to/your/utxo-monitor-data:/data \
  cardano-utxo-monitor:latest \
  --config-path /config/conf.json \
  --log4rs-path /config/log4rs.yaml \
  --host 0.0.0.0 \
  --port 8080
```

### Docker Compose

Make sure to have all the IPC socket and config files in the correct paths.

Also make sure to create the log directory and give it the correct permissions.

```bash
sh setup_logs.sh
```

```bash
docker-compose up -d
```

## API Endpoints

### POST /getUtxos

Query UTXOs for a specific public key hash.

**Request Body:**
```json
{
  "pkh": "YOUR_PUBLIC_KEY_HASH",
  "query": "Unspent",
  "offset": 0,
  "limit": 100
}
```

**Query Types:**

| Query | Purpose | Format |
|-------|---------|---------|
| `"Unspent"` | Get spendable UTXOs (most common) | `"Unspent"` |
| `{"All": null}` | Get all UTXOs since service start | `{"All": null}` |
| `{"All": 123456}` | Get all UTXOs from specific slot | `{"All": 123456}` |

**Examples:**

Get spendable UTXOs:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"pkh": "your_pkh", "query": "Unspent", "offset": 0, "limit": 100}'
```

Get transaction history:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"pkh": "your_pkh", "query": {"All": null}, "offset": 0, "limit": 100}'
```

## Development

Install rust and cargo


Install build dependencies

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev clang libc6-dev
```

Build the project
```bash
cargo build
```