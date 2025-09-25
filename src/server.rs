use crate::index::{CredentialKind, Txo, TxoQuery, UtxoResolver};
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::web::Data;
use actix_web::{guard, web, App, HttpResponse, HttpServer, Responder};
use async_primitives::beacon::Beacon;
use cml_chain::address::Address;
use cml_chain::certs::Credential;
use cml_chain::transaction::{DatumOption, ScriptRef, TransactionOutput};
use cml_core::serialization::Serialize;
use cml_crypto::{Ed25519KeyHash, RawBytesEncoding, ScriptHash, TransactionHash};
use spectrum_cardano_lib::transaction::TransactionOutputExtension;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum AddressQueryMode {
    #[default]
    ByPaymentCredential,
    ByStakingCredential,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsLookup {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    mode: AddressQueryMode,
    query: TxoQuery,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsRequest {
    #[serde(flatten)]
    lookup: GetTxOsLookup,
    offset: usize,
    limit: usize,
}

#[derive(Clone, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UTxO {
    pub transaction_hash: TransactionHash,
    pub index: usize,
    pub address: Address,
    pub value: Vec<Asset>,
    pub settled_at: Option<u64>,
    pub spent: bool,
    pub datum_hash: Option<String>,
    pub datum: Option<String>,
    pub script_ref: Option<ScriptRefDto>,
}

impl From<Txo> for UTxO {
    fn from(txo: Txo) -> Self {
        let (datum_hash, datum) = txo
            .output
            .datum()
            .map(|datum_option| match datum_option {
                DatumOption::Hash { datum_hash, .. } => (Some(datum_hash.to_raw_hex()), None),
                DatumOption::Datum { datum, .. } => {
                    let datum_hash = datum.hash().to_raw_hex();
                    let datum = hex::encode(datum.to_cbor_bytes());
                    (Some(datum_hash), Some(datum))
                }
            })
            .unwrap_or((None, None));

        let script_ref = match &txo.output {
            TransactionOutput::AlonzoFormatTxOut(_) => None,
            TransactionOutput::ConwayFormatTxOut(out) => out
                .script_reference
                .as_ref()
                .map(ScriptRefDto::from_script_ref),
        };

        Self {
            transaction_hash: txo.oref.tx_hash(),
            index: txo.oref.index() as usize,
            address: txo.output.address().clone(),
            value: vec![Asset {
                policy_id: "".to_string(),
                base16_name: "".to_string(),
                amount: txo.output.value().coin.to_string(),
            }]
            .into_iter()
            .chain(
                txo.output
                    .value()
                    .multiasset
                    .iter()
                    .flat_map(|(pol, assets)| {
                        assets.iter().map(|(name, amt)| Asset {
                            policy_id: pol.to_string(),
                            base16_name: name.to_raw_hex(),
                            amount: amt.to_string(),
                        })
                    }),
            )
            .collect(),
            settled_at: txo.settled_at,
            spent: txo.spent,
            datum_hash,
            datum,
            script_ref,
        }
    }
}

#[derive(Clone, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub policy_id: String,
    pub base16_name: String,
    pub amount: String,
}

#[derive(Clone, serde::Serialize, Debug)]
pub struct ScriptRefDto {
    #[serde(rename = "type")]
    pub script_type: ScriptType,
    pub script: String,
}

impl ScriptRefDto {
    fn from_script_ref(script_ref: &ScriptRef) -> Self {
        match script_ref {
            ScriptRef::Native { script, .. } => Self {
                script_type: ScriptType::Native,
                script: hex::encode(script.clone().to_cbor_bytes()),
            },
            ScriptRef::PlutusV1 { script, .. } => Self {
                script_type: ScriptType::PlutusV1,
                script: hex::encode(script.inner.clone()),
            },
            ScriptRef::PlutusV2 { script, .. } => Self {
                script_type: ScriptType::PlutusV2,
                script: hex::encode(script.inner.clone()),
            },
            ScriptRef::PlutusV3 { script, .. } => Self {
                script_type: ScriptType::PlutusV3,
                script: hex::encode(script.inner.clone()),
            },
        }
    }
}

#[derive(Clone, serde::Serialize, Debug)]
pub enum ScriptType {
    Native,
    PlutusV1,
    PlutusV2,
    PlutusV3,
}

pub struct Service<R>(PhantomData<R>);

fn credential_from_hash(hash_str: &str) -> Result<Credential, String> {
    if let Ok(hash) = Ed25519KeyHash::from_hex(hash_str) {
        Ok(Credential::PubKey {
            hash,
            len_encoding: Default::default(),
            tag_encoding: None,
            hash_encoding: Default::default(),
        })
    } else if let Ok(hash) = ScriptHash::from_hex(hash_str) {
        Ok(Credential::Script {
            hash,
            len_encoding: Default::default(),
            tag_encoding: None,
            hash_encoding: Default::default(),
        })
    } else {
        Err("Invalid key hash format. Expected hex-encoded hash.".to_string())
    }
}

fn resolve_request_credentials(
    address: Option<&String>,
    hash: Option<&String>,
    mode: AddressQueryMode,
) -> Result<(Credential, CredentialKind), String> {
    if let Some(address) = address {
        let address =
            Address::from_bech32(address).map_err(|_| "Invalid address format.".to_string())?;

        match mode {
            AddressQueryMode::ByPaymentCredential => match address.payment_cred() {
                Some(cred) => Ok((cred.clone(), CredentialKind::Payment)),
                None => Err("Address does not contain a payment credential.".to_string()),
            },
            AddressQueryMode::ByStakingCredential => match &address {
                Address::Base(base) => Ok((base.stake.clone(), CredentialKind::Stake)),
                Address::Reward(reward) => Ok((reward.payment.clone(), CredentialKind::Stake)),
                _ => Err(
                    "Address does not provide a staking credential for the requested mode."
                        .to_string(),
                ),
            },
        }
    } else if let Some(hash) = hash {
        let credential = credential_from_hash(hash)?;

        let kind = match mode {
            AddressQueryMode::ByPaymentCredential => CredentialKind::Payment,
            AddressQueryMode::ByStakingCredential => CredentialKind::Stake,
        };

        Ok((credential, kind))
    } else {
        Err("Either address or hash field is required.".to_string())
    }
}

async fn get_utxos<R>(req: web::Json<GetTxOsRequest>, db: Data<R>) -> impl Responder
where
    R: UtxoResolver + 'static,
{
    let GetTxOsRequest {
        lookup,
        offset,
        limit,
    } = req.into_inner();

    let (credential, kind) = match resolve_request_credentials(
        lookup.address.as_ref(),
        lookup.hash.as_ref(),
        lookup.mode.clone(),
    ) {
        Ok(result) => result,
        Err(err) => return HttpResponse::BadRequest().body(err),
    };

    let utxos = db
        .get_utxos(credential, kind, lookup.query, offset, limit)
        .await;
    let result = utxos.into_iter().map(UTxO::from).collect::<Vec<_>>();
    HttpResponse::Ok().json(result)
}

#[derive(Clone, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchResponseItem {
    pub request: GetTxOsLookup,
    pub utxos: Vec<UTxO>,
    pub error: Option<String>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchRequest {
    pub requests: Vec<GetTxOsLookup>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Clone, serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchResponse {
    pub offset: usize,
    pub limit: usize,
    pub responses: Vec<GetTxOsBatchResponseItem>,
}

async fn get_utxos_batch<R>(req: web::Json<GetTxOsBatchRequest>, db: Data<R>) -> impl Responder
where
    R: UtxoResolver + 'static,
{
    let GetTxOsBatchRequest {
        requests,
        offset,
        limit,
    } = req.into_inner();
    let mut responses = Vec::with_capacity(requests.len());

    for request in requests {
        match resolve_request_credentials(
            request.address.as_ref(),
            request.hash.as_ref(),
            request.mode.clone(),
        ) {
            Ok((credential, kind)) => {
                let utxos = db
                    .get_utxos(credential, kind, request.query, offset, limit)
                    .await;
                responses.push(GetTxOsBatchResponseItem {
                    request,
                    utxos: utxos.into_iter().map(UTxO::from).collect(),
                    error: None,
                });
            }
            Err(err) => responses.push(GetTxOsBatchResponseItem {
                request,
                utxos: Vec::new(),
                error: Some(err),
            }),
        }
    }

    HttpResponse::Ok().json(GetTxOsBatchResponse {
        offset,
        limit,
        responses,
    })
}

fn get_utxos_service<R: UtxoResolver + 'static>() -> actix_web::Resource {
    web::resource("/getUtxos").route(
        web::route()
            .guard(guard::Post())
            .guard(guard::Header("content-type", "application/json"))
            .to(get_utxos::<R>),
    )
}

fn get_utxos_batch_service<R: UtxoResolver + 'static>() -> actix_web::Resource {
    web::resource("/getUtxosBatch").route(
        web::route()
            .guard(guard::Post())
            .guard(guard::Header("content-type", "application/json"))
            .to(get_utxos_batch::<R>),
    )
}

async fn healthcheck(state_synced: Data<Beacon>) -> impl Responder {
    if state_synced.read() {
        HttpResponse::Ok().finish()
    } else {
        HttpResponse::ServiceUnavailable().finish()
    }
}

fn healthcheck_service() -> actix_web::Resource {
    web::resource("/health").route(web::route().guard(guard::Get()).to(healthcheck))
}

pub async fn build_api_server<R>(
    db: R,
    state_synced: Beacon,
    bind_addr: SocketAddr,
) -> Result<Server, io::Error>
where
    R: UtxoResolver + Send + Clone + 'static,
{
    Ok(HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header();

        App::new()
            .wrap(cors)
            .app_data(Data::new(db.clone()))
            .app_data(Data::new(state_synced.clone()))
            .service(healthcheck_service())
            .service(get_utxos_service::<R>())
            .service(get_utxos_batch_service::<R>())
    })
    .bind(bind_addr)?
    .workers(8)
    .disable_signals()
    .run())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn request_samples() {
        let sample_lookup_all = GetTxOsLookup {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByPaymentCredential,
            query: TxoQuery::All(Some(1)),
        };

        let sample_request_all = GetTxOsRequest {
            lookup: sample_lookup_all.clone(),
            offset: 0,
            limit: 10,
        };

        let json = serde_json::to_string_pretty(&sample_request_all).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let sample_lookup_unspent = GetTxOsLookup {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByStakingCredential,
            query: TxoQuery::Unspent,
        };

        let sample_request_unspent = GetTxOsRequest {
            lookup: sample_lookup_unspent.clone(),
            offset: 0,
            limit: 10,
        };

        let json = serde_json::to_string_pretty(&sample_request_unspent).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let batch_request = GetTxOsBatchRequest {
            requests: vec![sample_lookup_all.clone(), sample_lookup_unspent.clone()],
            offset: 0,
            limit: 10,
        };
        let batch_json = serde_json::to_string_pretty(&batch_request).unwrap();
        assert!(!batch_json.is_empty());
        println!("{}", batch_json);

        let response_item = GetTxOsBatchResponse {
            offset: 0,
            limit: 10,
            responses: vec![GetTxOsBatchResponseItem {
                request: sample_lookup_all,
                utxos: Vec::new(),
                error: Some("Some error".into()),
            }],
        };

        let response_json = serde_json::to_string_pretty(&response_item).unwrap();
        assert!(!response_json.is_empty());
        println!("{}", response_json);
    }
}
