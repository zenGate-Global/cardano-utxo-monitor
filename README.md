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
  "limit": 100,
  "includeCborHex": false
}
```

Set `"includeCborHex": true` to receive compact responses where each entry only
contains the transaction hash, output index, and canonical CBOR encoding of the
UTXO:

```json
[
  {
    "transactionHash": "<HASH>",
    "index": 0,
    "cbor": "<CBOR_HEX>"
  }
]
```

**Query Types:**

| Query | Purpose | Format |
|-------|---------|---------|
| `"Unspent"` | Get spendable UTXOs (most common) | `"Unspent"` |
| `{"All": null}` | Get all UTXOs since service start | `{"All": null}` |
| `{"All": 123456}` | Get all UTXOs from specific slot | `{"All": 123456}` |
| `{"unspentByUnit": "<policy_id><asset_name>"}` | Get spendable UTXOs that contain a specific asset unit (optionally scoped by address or hash) | `{"unspentByUnit": "<policy_id><asset_name>"}` |
| `{"byOutputRef": {"txHash": "<tx_hash>", "index": 0}}` | Fetch a specific output by transaction hash and index | `{"byOutputRef": {"txHash": "<tx_hash>", "index": 0}}` |

When `address`/`hash` are omitted in combination with `unspentByUnit`, the monitor scans all stored unspent outputs and returns only those containing the requested asset unit.

**Examples:**

Get spendable UTXOs:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"pkh": "your_pkh", "query": "Unspent", "offset": 0, "limit": 100}'
```

Get unspent UTXOs containing a specific unit scoped to a payment credential:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"pkh": "your_pkh", "query": {"unspentByUnit": "policyidassetname"}, "offset": 0, "limit": 100}'
```

Get unspent UTXOs containing a specific unit across all known credentials:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"query": {"unspentByUnit": "policyidassetname"}, "offset": 0, "limit": 100}'
```

Get a specific UTXO by transaction hash and output index:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"query": {"byOutputRef": {"txHash": "<tx_hash>", "index": 0}}, "offset": 0, "limit": 1, "includeCborHex": true}'
```

Get transaction history:
```bash
curl -X POST http://localhost:8080/getUtxos \
  -H "Content-Type: application/json" \
  -d '{"pkh": "your_pkh", "query": {"All": null}, "offset": 0, "limit": 100}'
```

### POST /getUtxosBatch

Query multiple UTXO options in a single request. Provide the shared pagination values
once and supply individual lookup parameters for each entry.

**Request Body:**
```json
{
  "requests": [
    {
      "address": "addr_test1...",
      "mode": "byPaymentCredential",
      "query": "Unspent"
    },
    {
      "hash": "stake_test1...",
      "mode": "byStakingCredential",
      "query": {"All": null}
    },
    {
      "address": "addr_test1...",
      "mode": "byPaymentCredential",
      "query": {"unspentByUnit": "policyidassetname"}
    },
    {
      "query": {"unspentByUnit": "policyidassetname"}
    }
  ],
  "offset": 0,
  "limit": 50,
  "includeCborHex": true
}
```

**Response:**
```json
{
  "offset": 0,
  "limit": 50,
  "responses": [
    {
      "request": { "address": "addr_test1...", "mode": "byPaymentCredential", "query": "Unspent" },
      "utxos": [ /* ... */ ],
      "error": null
    },
    {
      "request": { "hash": "stake_test1...", "mode": "byStakingCredential", "query": {"All": null} },
      "utxos": [ /* ... */ ],
      "error": "Optional error message if the request failed"
    }
  ]
}
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
