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
use std::net::SocketAddr;
use utoipa::OpenApi;
use utoipa::ToSchema;
use utoipa_swagger_ui::SwaggerUi;

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, Default, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum AddressQueryMode {
    #[default]
    ByPaymentCredential,
    ByStakingCredential,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApiTxoQuery {
    All(Option<u64>),
    Unspent,
    UnspentByUnit(String),
}

impl From<ApiTxoQuery> for TxoQuery {
    fn from(value: ApiTxoQuery) -> Self {
        match value {
            ApiTxoQuery::All(slot) => TxoQuery::All(slot),
            ApiTxoQuery::Unspent => TxoQuery::Unspent,
            ApiTxoQuery::UnspentByUnit(unit) => TxoQuery::UnspentByUnit(unit),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsLookup {
    #[serde(default)]
    #[schema(nullable = true, example = "addr_test1...")]
    address: Option<String>,
    #[serde(default)]
    #[schema(nullable = true, example = "f1c2...")]
    hash: Option<String>,
    #[serde(default)]
    mode: AddressQueryMode,
    query: ApiTxoQuery,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsRequest {
    #[serde(flatten)]
    lookup: GetTxOsLookup,
    offset: usize,
    limit: usize,
    #[serde(default)]
    include_cbor_hex: bool,
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UTxO {
    #[schema(value_type = String, example = "a1b2c3...")]
    pub transaction_hash: TransactionHash,
    pub index: usize,
    #[schema(value_type = String, example = "addr1...")]
    pub address: Address,
    pub value: Vec<Asset>,
    pub settled_at: Option<u64>,
    pub spent: bool,
    pub datum_hash: Option<String>,
    pub datum: Option<String>,
    pub script_ref: Option<ScriptRefDto>,
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UTxOCbor {
    #[schema(value_type = String, example = "a1b2c3...")]
    pub transaction_hash: TransactionHash,
    pub index: usize,
    pub cbor: String,
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

impl From<Txo> for UTxOCbor {
    fn from(txo: Txo) -> Self {
        Self {
            transaction_hash: txo.oref.tx_hash(),
            index: txo.oref.index() as usize,
            cbor: hex::encode(txo.output.to_cbor_bytes()),
        }
    }
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(untagged)]
pub enum UTxOResponse {
    Detailed(UTxO),
    Compact(UTxOCbor),
}

impl UTxOResponse {
    fn detailed(txo: Txo) -> Self {
        UTxOResponse::Detailed(UTxO::from(txo))
    }

    fn compact(txo: Txo) -> Self {
        UTxOResponse::Compact(UTxOCbor::from(txo))
    }
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub policy_id: String,
    pub base16_name: String,
    pub amount: String,
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
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

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
pub enum ScriptType {
    Native,
    PlutusV1,
    PlutusV2,
    PlutusV3,
}

#[derive(OpenApi)]
#[openapi(
    paths(healthcheck, get_utxos, get_utxos_batch),
    components(schemas(
        AddressQueryMode,
        ApiTxoQuery,
        GetTxOsLookup,
        GetTxOsRequest,
        UTxO,
        UTxOCbor,
        UTxOResponse,
        Asset,
        ScriptRefDto,
        ScriptType,
        GetTxOsBatchRequest,
        GetTxOsBatchResponse,
        GetTxOsBatchResponseItem
    )),
    tags(
        (name = "Health", description = "Service health endpoints"),
        (name = "UTxO", description = "UTxO lookup endpoints")
    )
)]
pub struct ApiDoc;

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
    query: &ApiTxoQuery,
) -> Result<Option<(Credential, CredentialKind)>, String> {
    if let Some(address) = address {
        let address =
            Address::from_bech32(address).map_err(|_| "Invalid address format.".to_string())?;

        match mode {
            AddressQueryMode::ByPaymentCredential => match address.payment_cred() {
                Some(cred) => Ok(Some((cred.clone(), CredentialKind::Payment))),
                None => Err("Address does not contain a payment credential.".to_string()),
            },
            AddressQueryMode::ByStakingCredential => match &address {
                Address::Base(base) => Ok(Some((base.stake.clone(), CredentialKind::Stake))),
                Address::Reward(reward) => {
                    Ok(Some((reward.payment.clone(), CredentialKind::Stake)))
                }
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

        Ok(Some((credential, kind)))
    } else {
        match query {
            ApiTxoQuery::UnspentByUnit(_) => Ok(None),
            _ => Err("Either address or hash field is required unless querying unspentByUnit without a credential.".to_string()),
        }
    }
}

#[utoipa::path(
    post,
    path = "/getUtxos",
    request_body = GetTxOsRequest,
    responses(
        (status = 200, description = "Found matching UTxOs", body = [UTxOResponse]),
        (status = 400, description = "Invalid request", body = String)
    ),
    tag = "UTxO"
)]
async fn get_utxos<R>(req: web::Json<GetTxOsRequest>, db: Data<R>) -> impl Responder
where
    R: UtxoResolver + 'static,
{
    let GetTxOsRequest {
        lookup,
        offset,
        limit,
        include_cbor_hex,
    } = req.into_inner();

    let query_api = lookup.query.clone();

    let scope = match resolve_request_credentials(
        lookup.address.as_ref(),
        lookup.hash.as_ref(),
        lookup.mode,
        &query_api,
    ) {
        Ok(result) => result,
        Err(err) => return HttpResponse::BadRequest().body(err),
    };

    let query: TxoQuery = query_api.into();

    let utxos = db.get_utxos(scope, query, offset, limit).await;
    let result = utxos
        .into_iter()
        .map(|txo| {
            if include_cbor_hex {
                UTxOResponse::compact(txo)
            } else {
                UTxOResponse::detailed(txo)
            }
        })
        .collect::<Vec<_>>();
    HttpResponse::Ok().json(result)
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchResponseItem {
    pub request: GetTxOsLookup,
    pub utxos: Vec<UTxOResponse>,
    pub error: Option<String>,
}

#[derive(Clone, serde::Deserialize, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchRequest {
    pub requests: Vec<GetTxOsLookup>,
    pub offset: usize,
    pub limit: usize,
    #[serde(default)]
    pub include_cbor_hex: bool,
}

#[derive(Clone, serde::Serialize, Debug, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GetTxOsBatchResponse {
    pub offset: usize,
    pub limit: usize,
    pub responses: Vec<GetTxOsBatchResponseItem>,
}

#[utoipa::path(
    post,
    path = "/getUtxosBatch",
    request_body = GetTxOsBatchRequest,
    responses(
        (status = 200, description = "Batch UTxO lookup result", body = GetTxOsBatchResponse)
    ),
    tag = "UTxO"
)]
async fn get_utxos_batch<R>(req: web::Json<GetTxOsBatchRequest>, db: Data<R>) -> impl Responder
where
    R: UtxoResolver + 'static,
{
    let GetTxOsBatchRequest {
        requests,
        offset,
        limit,
        include_cbor_hex,
    } = req.into_inner();
    let mut responses = Vec::with_capacity(requests.len());

    for request in requests {
        match resolve_request_credentials(
            request.address.as_ref(),
            request.hash.as_ref(),
            request.mode.clone(),
            &request.query,
        ) {
            Ok(scope) => {
                let query: TxoQuery = request.query.clone().into();
                let utxos = db.get_utxos(scope, query, offset, limit).await;
                let utxos = utxos
                    .into_iter()
                    .map(|txo| {
                        if include_cbor_hex {
                            UTxOResponse::compact(txo)
                        } else {
                            UTxOResponse::detailed(txo)
                        }
                    })
                    .collect();
                responses.push(GetTxOsBatchResponseItem {
                    request,
                    utxos,
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

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Node is synced and serving requests"),
        (status = 503, description = "Node is syncing")
    ),
    tag = "Health"
)]
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
    let openapi = ApiDoc::openapi();

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
            .service(SwaggerUi::new("/docs/{_:.*}").url("/docs/openapi.json", openapi.clone()))
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
            query: ApiTxoQuery::All(Some(1)),
        };

        let sample_request_all = GetTxOsRequest {
            lookup: sample_lookup_all.clone(),
            offset: 0,
            limit: 10,
            include_cbor_hex: false,
        };

        let json = serde_json::to_string_pretty(&sample_request_all).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let sample_lookup_unspent = GetTxOsLookup {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByStakingCredential,
            query: ApiTxoQuery::Unspent,
        };

        let sample_request_unspent = GetTxOsRequest {
            lookup: sample_lookup_unspent.clone(),
            offset: 0,
            limit: 10,
            include_cbor_hex: false,
        };

        let json = serde_json::to_string_pretty(&sample_request_unspent).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let sample_lookup_unspent_by_unit = GetTxOsLookup {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByPaymentCredential,
            query: ApiTxoQuery::UnspentByUnit(
                "policyid000000000000000000000000000000000000000000asset".into(),
            ),
        };

        let sample_lookup_unspent_by_unit_global = GetTxOsLookup {
            address: None,
            hash: None,
            mode: AddressQueryMode::ByPaymentCredential,
            query: ApiTxoQuery::UnspentByUnit(
                "policyid000000000000000000000000000000000000000000asset".into(),
            ),
        };

        let sample_request_unspent_by_unit = GetTxOsRequest {
            lookup: sample_lookup_unspent_by_unit.clone(),
            offset: 0,
            limit: 10,
            include_cbor_hex: true,
        };

        let json = serde_json::to_string_pretty(&sample_request_unspent_by_unit).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let batch_request = GetTxOsBatchRequest {
            requests: vec![
                sample_lookup_all.clone(),
                sample_lookup_unspent.clone(),
                sample_lookup_unspent_by_unit.clone(),
                sample_lookup_unspent_by_unit_global.clone(),
            ],
            offset: 0,
            limit: 10,
            include_cbor_hex: true,
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
