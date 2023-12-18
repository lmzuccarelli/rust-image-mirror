// use modules
use clap::Parser;
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
mod release;

// use local modules
use api::schema::*;
use config::read::*;
use diff::metadata_cache::*;
use log::logging::*;
use operator::collector::*;
use release::collector::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let cfg = args.config.as_ref().unwrap().to_string();
    let level = args.loglevel.unwrap().to_string();
    let skip = args.skip.unwrap().to_string();

    // convert to enum
    let res_log_level = match level.as_str() {
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
    };

    // convert to enum
    let res_skip = match skip.as_str() {
        "release" => Skip::RELEASE,
        "operators" => Skip::OPERATORS,
        "additional" => Skip::ADDITIONAL,
        "release-operators" => Skip::RELOPS,
        _ => Skip::NONE,
    };

    // setup logging
    let log = &Logging {
        log_level: res_log_level,
    };

    // check that destination is set correctly
    if args.destination == "" {
        log.error("destination is mandatory use docker:// or file:// prefix");
        std::process::exit(exitcode::USAGE);
    }

    log.info(&format!("rust-image-mirror {} ", cfg));
    if res_skip != Skip::NONE {
        log.hi(&format!("skipping {}", skip));
    }
    let mut current_cache = HashSet::new();

    if args.diff_tar.unwrap() {
        if args.date.clone().unwrap().len() == 0 {
            current_cache = get_metadata_dirs_incremental(log, String::from("working-dir/"));
            log.debug(&format!("current cache {:#?} ", current_cache));
        }
    }

    // Parse the config serde_yaml::ImageSetConfiguration.
    let config = load_config(cfg).unwrap();
    let isc_config = parse_yaml_config(config.clone()).unwrap();
    log.debug(&format!(
        "image set config operators {:#?}",
        isc_config.mirror.operators
    ));

    // initialize the client request interface
    let reg_con = ImplRegistryInterface {};

    // this is mirrorToDisk
    if args.destination.contains("file://") {
        // check for release image
        if isc_config.mirror.release.is_some()
            && res_skip != Skip::RELEASE
            && res_skip != Skip::RELOPS
        {
            release_mirror_to_disk(
                reg_con.clone(),
                log,
                String::from("./working-dir/"),
                isc_config.mirror.release.unwrap(),
            )
            .await;
        }
        // check for operators
        if isc_config.mirror.operators.is_some()
            && res_skip != Skip::OPERATORS
            && res_skip != Skip::RELOPS
        {
            operator_mirror_to_disk(
                reg_con.clone(),
                log,
                String::from("./working-dir/"),
                isc_config.mirror.operators.unwrap(),
            )
            .await;
        }

        // TODO: call additionalImages collector

        // if flag diff-tar is set create a diff tar.gz
        if args.diff_tar.unwrap() {
            let mut new_cache = HashSet::new();
            log.trace(&format!("new cache {:#?}", new_cache));
            if args.date.clone().unwrap().len() > 0 {
                new_cache = get_metadata_dirs_by_date(
                    log,
                    String::from("working-dir/"),
                    args.date.unwrap(),
                );
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
    } else {
        // this is diskToMirror
        let destination = args.destination;
        if res_skip != Skip::RELEASE && res_skip != Skip::RELOPS {
            release_disk_to_mirror(
                reg_con.clone(),
                log,
                String::from("./working-dir/"),
                destination.clone(),
                isc_config.mirror.release.unwrap(),
            )
            .await;
        }

        if res_skip != Skip::OPERATORS && res_skip != Skip::RELOPS {
            operator_disk_to_mirror(
                reg_con.clone(),
                log,
                String::from("./working-dir/"),
                destination.clone(),
                isc_config.mirror.operators.unwrap(),
            )
            .await;
        }
    }
}
