//use auth::credentials::get_token;
//use batch::copy::get_blobs;
// use modules
use clap::Parser;
use operator::collector::mirror_to_disk;
use std::collections::HashSet;
use std::path::Path;
use tokio;

// define local modules
mod api;
mod auth;
mod batch;
mod config;
mod diff;
mod index;
mod log;
mod manifests;
mod operator;

// use local modules
use api::schema::*;
use config::read::*;
use diff::metadata_cache::*;
use log::logging::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let cfg = args.config.as_ref().unwrap().to_string();
    let level = args.loglevel.unwrap().to_string();

    // convert to enum
    let res = match level.as_str() {
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
    };

    let log = &Logging { log_level: res };

    log.info(&format!("rust-image-mirror {} ", cfg));
    let mut current_cache = HashSet::new();

    if args.diff_tar.unwrap() {
        if args.date.clone().unwrap().len() == 0 {
            current_cache = get_metadata_dirs_incremental(log, String::from("working-dir/"));
            log.mid(&format!("current cache {:#?} ", current_cache));
        }
    }

    // Parse the config serde_yaml::ImageSetConfiguration.
    let config = load_config(cfg).unwrap();
    let isc_config = parse_yaml_config(config.clone()).unwrap();
    log.debug(&format!(
        "image set config operators {:#?}",
        isc_config.mirror.operators
    ));

    // TODO: call release collector

    let reg_con = ImplRegistryInterface {};

    mirror_to_disk(
        reg_con,
        log,
        String::from("./working-dir/"),
        String::from(""),
        isc_config.mirror.operators.unwrap(),
    )
    .await;

    // TODO: call additionalImages collector

    // if flag diff-tar is set create a diff tar.gz
    if args.diff_tar.unwrap() {
        let mut new_cache = HashSet::new();
        log.trace(&format!("new cache {:#?}", new_cache));
        if args.date.clone().unwrap().len() > 0 {
            new_cache =
                get_metadata_dirs_by_date(log, String::from("working-dir/"), args.date.unwrap());
        } else {
            new_cache = get_metadata_dirs_incremental(log, String::from("working-dir/"));
        }
        let diff: Vec<_> = new_cache.difference(&current_cache).collect();
        log.mid(&format!("difference {:#?}", diff));
        if diff.len() > 0 {
            log.info("creating mirror_diff.tar.gz");
            let res = create_diff_tar(
                log,
                String::from("mirror-diff.tar.gz"),
                String::from("working-dir/blobs-store"),
                diff,
                config,
            );
            match res {
                Ok(_) => log.info("mirror-diff.tar.gz successfully created"),
                Err(err) => log.error(&format!("errror creating diff tar {:#?}", err)),
            }
        } else {
            log.info("no difference found mirror_diff.tar.gz not created");
        }
    }
}
