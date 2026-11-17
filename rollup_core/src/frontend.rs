use std::str::FromStr;

use actix_web::{error, web, HttpResponse};
use base64::{prelude::BASE64_STANDARD, Engine};
use serde::Deserialize;
use serde_json::json;
use solana_pubkey::Pubkey;
use solana_signature::Signature;
use tokio::sync::{mpsc, oneshot};

use crate::sequencer::{SequencerMessage, SubmissionError};

pub(crate) struct Frontend {
    pub sequencer: mpsc::Sender<SequencerMessage>,
}

#[derive(Deserialize)]
pub(crate) struct GetTransaction {
    get_tx: String,
}

#[derive(Deserialize)]
pub(crate) struct RollupTransaction {
    // Base64 of the serialized transaction, the form Solana's `sendTransaction` takes.
    // The legacy `sender` label is accepted as extra JSON, never used as authentication.
    sol_transaction: String,
}

pub(crate) async fn submit_transaction(
    body: web::Json<RollupTransaction>,
    frontend: web::Data<Frontend>,
) -> actix_web::Result<HttpResponse> {
    let Ok(wire_transaction) = BASE64_STANDARD.decode(&body.sol_transaction) else {
        return Ok(HttpResponse::BadRequest()
            .json(json!({"error": "sol_transaction must be a base64 serialized transaction"})));
    };

    let (reply, response) = oneshot::channel();
    frontend
        .sequencer
        .send(SequencerMessage::Submit {
            wire_transaction,
            reply,
        })
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer unavailable"))?;
    match response
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer stopped"))?
    {
        Ok(receipt) => Ok(HttpResponse::Ok().json(receipt)),
        Err(SubmissionError::Invalid(reason)) => {
            Ok(HttpResponse::BadRequest().json(json!({"error": reason})))
        }
        Err(SubmissionError::AlreadyProcessed) => {
            Ok(HttpResponse::Conflict().json(json!({"error": "transaction already processed"})))
        }
        Err(SubmissionError::Rejected(reason)) => {
            Ok(HttpResponse::UnprocessableEntity().json(json!({"error": reason})))
        }
        Err(SubmissionError::ClusterUnavailable(reason)) => {
            Ok(HttpResponse::ServiceUnavailable().json(json!({"error": reason})))
        }
    }
}

pub(crate) async fn get_transaction(
    body: web::Json<GetTransaction>,
    frontend: web::Data<Frontend>,
) -> actix_web::Result<HttpResponse> {
    let Ok(signature) = Signature::from_str(&body.get_tx) else {
        return Ok(
            HttpResponse::BadRequest().json(json!({"error": "invalid transaction signature"}))
        );
    };
    let (reply, response) = oneshot::channel();
    frontend
        .sequencer
        .send(SequencerMessage::Transaction { signature, reply })
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer unavailable"))?;
    match response
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer stopped"))?
    {
        Some(receipt) => Ok(HttpResponse::Ok().json(receipt)),
        None => Ok(HttpResponse::NotFound().json(json!({"error": "transaction not found"}))),
    }
}

pub(crate) async fn get_account(
    address: web::Path<String>,
    frontend: web::Data<Frontend>,
) -> actix_web::Result<HttpResponse> {
    let Ok(address) = Pubkey::from_str(&address) else {
        return Ok(HttpResponse::BadRequest().json(json!({"error": "invalid account address"})));
    };
    let (reply, response) = oneshot::channel();
    frontend
        .sequencer
        .send(SequencerMessage::Balance { address, reply })
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer unavailable"))?;
    match response
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer stopped"))?
    {
        Ok(Some(balance)) => Ok(HttpResponse::Ok().json(balance)),
        Ok(None) => Ok(HttpResponse::NotFound().json(json!({"error": "account not found"}))),
        Err(reason) => Ok(HttpResponse::ServiceUnavailable().json(json!({"error": reason}))),
    }
}

pub(crate) async fn get_latest_blockhash(
    frontend: web::Data<Frontend>,
) -> actix_web::Result<HttpResponse> {
    let (reply, response) = oneshot::channel();
    frontend
        .sequencer
        .send(SequencerMessage::Blockhash { reply })
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer unavailable"))?;
    let blockhash = response
        .await
        .map_err(|_| error::ErrorServiceUnavailable("sequencer stopped"))?;
    Ok(HttpResponse::Ok().json(json!({"blockhash": blockhash.to_string()})))
}

pub(crate) async fn health() -> HttpResponse {
    HttpResponse::Ok().json(
        json!({"status": "ok", "application": "sol_transfer", "settlement": "not_configured"}),
    )
}
