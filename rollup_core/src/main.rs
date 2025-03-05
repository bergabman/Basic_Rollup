use std::thread;
use std::sync::{Arc, RwLock};
use crate::delegation_service::DelegationService;

use actix_web::{web, App, HttpResponse, HttpServer};
use async_channel;
use frontend::{FrontendMessage, RollupTransaction, TransactionResponse};
use rollupdb::{RollupDB, RollupDBMessage};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::{account::AccountSharedData, transaction::Transaction};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use crossbeam;
mod frontend;
mod rollupdb;
mod sequencer;
mod settle;
mod processor;
mod loader;
mod bundler;
mod errors;
mod delegation;
mod delegation_service;

// #[actix_web::main]
// #[tokio::main]
fn main() { // async
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

    log::info!("starting HTTP server at http://localhost:8080");

    let (sequencer_sender, sequencer_receiver) = 
        crossbeam::channel::unbounded::<Transaction>();
    let (rollupdb_sender, rollupdb_receiver) = crossbeam::channel::unbounded::<RollupDBMessage>();

    
    let (frontend_sender, frontend_receiver) = async_channel::unbounded::<FrontendMessage>(); // Channel for communication between data availability layer and frontend
    pub type PubkeyAccountSharedData = Option<Vec<(Pubkey, AccountSharedData)>>;
    let (account_sender, account_receiver) = async_channel::unbounded::<PubkeyAccountSharedData>();
    let (sender_locked_account, receiver_locked_account) = async_channel::unbounded::<bool>();

    let db_sender2 = rollupdb_sender.clone();
    let fe_2 = frontend_sender.clone();


    let signer = Keypair::new(); // Temporary keypair, will be replaced when client connects
    
    let (delegation_keypair_sender, delegation_keypair_receiver) = async_channel::unbounded::<Vec<u8>>();
    
    let delegation_service = Arc::new(RwLock::new(
        DelegationService::new("https://api.devnet.solana.com", signer)
    ));

    let delegation_service_clone = delegation_service.clone();
    let delegation_service_clone_1 = delegation_service.clone();
    

    let asdserver_thread = thread::spawn(|| {
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_time()
            .build()
            .unwrap();

        rt.spawn(async {
            sequencer::run(
                sequencer_receiver,
                db_sender2,
                account_receiver,
                receiver_locked_account,
                delegation_service_clone,
            ).await.unwrap()
        });

        
        rt.block_on(RollupDB::run(rollupdb_receiver, fe_2, account_sender, sender_locked_account, delegation_service_clone_1));
    });
   

     // Spawn the Actix Web server in a separate thread
    let server_thread = thread::spawn( || {
            // Create a separate Tokio runtime for Actix Web
        let rt2 = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_io()
            .enable_time()
            .build()
            .unwrap();

        // Create frontend server
        rt2.block_on(async {HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(sequencer_sender.clone()))
                .app_data(web::Data::new(rollupdb_sender.clone()))
                .app_data(web::Data::new(frontend_sender.clone()))
                .app_data(web::Data::new(frontend_receiver.clone()))
                .route("/", web::get().to(frontend::test))
                .route(
                    "/get_transaction",
                    web::post().to(frontend::get_transaction),
                )
                .route(
                    "/submit_transaction",
                    web::post().to(frontend::submit_transaction),
                )
                .route(
                    "/init_delegation_service",
                    {
                        let delegation_service = delegation_service.clone();
                        web::post().to(move |body: web::Bytes| {
                            let keypair = Keypair::from_bytes(&body).unwrap();
                            *delegation_service.write().unwrap() = 
                                DelegationService::new("https://api.devnet.solana.com", keypair);
                            log::info!("Delegation service initialized___");
                            // log::info!("{:?}", )
                            HttpResponse::Ok()
                        })
                    },
                )
                .route(
                    "/add_delegation_signer",
                    {
                        let delegation_service = delegation_service.clone();
                        web::post().to(move |body: web::Bytes| {
                            let keypair = Keypair::from_bytes(&body).unwrap();
                            delegation_service.write().unwrap().add_signer(keypair);
                            log::info!("Added signer to delegation service");
                            HttpResponse::Ok()
                        })
                    },
                )
        
        })
        .worker_max_blocking_threads(2)
        .bind("127.0.0.1:8080")
        .unwrap()
        .run()
        .await
        .unwrap();
        });
        });
        server_thread.join().unwrap();

}
