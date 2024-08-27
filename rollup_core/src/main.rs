use actix_web::{web, App, HttpServer};
use async_channel;
use frontend::FrontendMessage;
use rollupdb::{RollupDB, RollupDBMessage};
use solana_sdk::transaction::Transaction;

mod frontend;
mod rollupdb;
mod sequencer;
mod settle;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("debug"));

    log::info!("starting HTTP server at http://localhost:8080");

    let (sequencer_sender, sequencer_receiver) = async_channel::bounded::<Transaction>(100); // Channel for communication between frontend and sequencer
    let (rollupdb_sender, rollupdb_receiver) = async_channel::unbounded::<RollupDBMessage>(); // Channel for communication between sequencer and accountsdb
    let (frontend_sender, frontend_receiver) = async_channel::unbounded::<FrontendMessage>(); // Channel for communication between data availability layer and frontend

    // Create sequencer task
    tokio::spawn(sequencer::run(sequencer_receiver, rollupdb_sender.clone()));

    // Create rollup db task (accounts + transactions)
    tokio::spawn(RollupDB::run(rollupdb_receiver, frontend_sender.clone()));

    // let frontend_receiver_mutex = Arc::new(Mutex::new(frontend_receiver));

    // Create frontend server
    HttpServer::new(move || {
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
        // .service(
        //     web::resource("/submit_transaction")
        //         .route(web::post().to(frontend::submit_transaction)),
        // )
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
