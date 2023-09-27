//use auth::credentials::get_token;
//use batch::copy::get_blobs;
// use modules
use clap::Parser;
use operator::collector::mirror_to_disk;
use std::path::Path;
use tokio;

// define local modules
mod api;
mod auth;
mod batch;
mod config;
mod index;
mod log;
mod manifests;
mod operator;

// use local modules
use api::schema::*;
use config::read::*;
use log::logging::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let cfg = args.config.as_ref().unwrap().to_string();

    let log = &Logging {
        log_level: Level::DEBUG,
    };

    log.info(&format!("rust-image-mirror {} ", cfg));

    // Parse the config serde_yaml::ImageSetConfiguration.
    let config = load_config(cfg).unwrap();
    let isc_config = parse_yaml_config(config).unwrap();
    log.debug(&format!("{:#?}", isc_config.mirror.operators));

    // TODO: call release collector

    // call operator collector
    // let token = get_token(log,isc_config.mirror.operators)
    mirror_to_disk(log, isc_config.mirror.operators.unwrap()).await;
    // get_blobs(log, &"", token, images)

    // TODO: call additionalImages collector

    // let op_url = get_blobs_url_by_string(img.image.clone());
    // get_blobs(log, op_url, token.clone(), fslayers.clone()).await;
}
