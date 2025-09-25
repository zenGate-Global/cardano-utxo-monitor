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
pub struct GetTxOsRequest {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    hash: Option<String>,
    #[serde(default)]
    mode: AddressQueryMode,
    query: TxoQuery,
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

async fn get_utxos<R>(req: web::Json<GetTxOsRequest>, db: Data<R>) -> impl Responder
where
    R: UtxoResolver + 'static,
{
    let GetTxOsRequest {
        address,
        hash,
        mode,
        query,
        offset,
        limit,
    } = req.into_inner();

    let (credential, kind) = if let Some(address) = address {
        let address = match Address::from_bech32(&address) {
            Ok(addr) => addr,
            Err(_) => return HttpResponse::BadRequest().body("Invalid address format."),
        };

        match mode {
            AddressQueryMode::ByPaymentCredential => match address.payment_cred() {
                Some(cred) => (cred.clone(), CredentialKind::Payment),
                None => {
                    return HttpResponse::BadRequest()
                        .body("Address does not contain a payment credential.");
                }
            },
            AddressQueryMode::ByStakingCredential => match &address {
                Address::Base(base) => (base.stake.clone(), CredentialKind::Stake),
                Address::Reward(reward) => (reward.payment.clone(), CredentialKind::Stake),
                _ => {
                    return HttpResponse::BadRequest().body(
                        "Address does not provide a staking credential for the requested mode.",
                    );
                }
            },
        }
    } else if let Some(hash) = hash {
        let credential = match credential_from_hash(&hash) {
            Ok(credential) => credential,
            Err(err) => return HttpResponse::BadRequest().body(err),
        };

        let kind = match mode {
            AddressQueryMode::ByPaymentCredential => CredentialKind::Payment,
            AddressQueryMode::ByStakingCredential => CredentialKind::Stake,
        };

        (credential, kind)
    } else {
        return HttpResponse::BadRequest().body("Either address or hash field is required.");
    };

    let utxos = db.get_utxos(credential, kind, query, offset, limit).await;
    let result = utxos.into_iter().map(UTxO::from).collect::<Vec<_>>();
    HttpResponse::Ok().json(result)
}

fn get_utxos_service<R: UtxoResolver + 'static>() -> actix_web::Resource {
    web::resource("/getUtxos").route(
        web::route()
            .guard(guard::Post())
            .guard(guard::Header("content-type", "application/json"))
            .to(get_utxos::<R>),
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
        let sample_request_all = GetTxOsRequest {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByPaymentCredential,
            query: TxoQuery::All(Some(1)),
            offset: 0,
            limit: 10,
        };

        let json = serde_json::to_string_pretty(&sample_request_all).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);

        let sample_request_unspent = GetTxOsRequest {
            address: Some("addr_test1qpz8h9w8sample000000000000000000000000000000000000".into()),
            hash: None,
            mode: AddressQueryMode::ByStakingCredential,
            query: TxoQuery::Unspent,
            offset: 0,
            limit: 10,
        };

        let json = serde_json::to_string_pretty(&sample_request_unspent).unwrap();
        assert!(!json.is_empty());
        println!("{}", json);
    }
}
