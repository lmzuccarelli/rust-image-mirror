// use modules
use crate::additional::collector::*;
use crate::operator::collector::*;
use crate::release::collector::*;
use clap::Parser;
use custom_logger::*;
use mirror_copy::ImplRegistryInterface;
use std::fs;
use std::process;
use tokio;

// define local modules
mod additional;
mod api;
mod archive;
mod clusterresources;
mod config;
mod error;
mod image;
mod operator;
mod release;
mod removable_media;

// use local modules
use api::schema::*;
//use clusterresources::*;
use archive::metadata_cache::*;
use config::load::*;
use removable_media::collector::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let cfg = args.config.as_ref().unwrap().to_string();
    let level = args.loglevel.unwrap().to_string();
    let skip_manifests = args.skip_manifest_check.unwrap().to_string();
    let dry_run = args.dry_run;

    // convert to enum
    let res_log_level = match level.as_str() {
        "info" => Level::INFO,
        "debug" => Level::DEBUG,
        "trace" => Level::TRACE,
        _ => Level::INFO,
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

    log.debug(&format!("image-mirror config file {} ", cfg));

    // Parse the config serde_yaml::ImageSetConfiguration.
    let config = load_config(cfg).unwrap();
    let isc_config = parse_yaml_config(config.clone()).unwrap();

    log.debug(&format!(
        "image set config releases {:#?}",
        isc_config.mirror.release
    ));

    log.debug(&format!(
        "image set config operators {:#?}",
        isc_config.mirror.operators
    ));

    log.debug(&format!(
        "image set config additional images {:#?}",
        isc_config.mirror.additional_images
    ));

    // initialize the client request interface
    let reg_con = ImplRegistryInterface {};

    // this is mirrorToDisk
    if args.destination.contains("file://") {
        let destination = args.destination.split("file://").nth(1).unwrap();
        fs::create_dir_all(&format!(
            "{}/{}",
            destination,
            "working-dir/mirror-metadata".to_string()
        ))
        .expect("should create manifests directory");

        fs::create_dir_all(&format!("{}/{}", destination, "mappings".to_string()))
            .expect("should create mappings directory");

        // check for release images
        let skip_manifest_check = skip_manifests == "release" || skip_manifests == "all";
        if isc_config.mirror.release.is_some() {
            release_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config.mirror.release.unwrap(),
            )
            .await;
        }
        // check for operators
        let skip_manifest_check = skip_manifests == "operators" || skip_manifests == "all";
        if isc_config.mirror.operators.is_some() {
            operator_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config.mirror.operators.unwrap(),
            )
            .await;
        }
        // check for additional images
        let skip_manifest_check = skip_manifests == "additional";
        if isc_config.mirror.additional_images.is_some() {
            additional_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config.mirror.additional_images.unwrap(),
            )
            .await;
        }

        if !dry_run {
            // finally create tar archive
            log.info("creating tar files");
            let res = create_tar(log, destination.to_string());
            match res {
                Ok(_) => {
                    log.info("tar files successfully created");
                    process::exit(0);
                }
                Err(err) => {
                    log.error(&format!("errror creating tar {:#?}", err));
                    process::exit(1);
                }
            }
        }
    } else {
        // this is disk-to-mirror
        let destination_registry = args.destination;
        if !destination_registry.contains("docker://") {
            log.error("destination disk-to-mirror must have docker:// prefix");
            process::exit(exitcode::USAGE);
        }
        if args.from.len() == 0 {
            log.error("from flag with protocol file:// must be set in disk-to-mirror mode");
            process::exit(exitcode::USAGE);
        } else {
            if !args.from.contains("file://") {
                log.error("from director with protocolg must have file::// prefix");
                process::exit(exitcode::USAGE);
            }
        }
        let from = args.from.split("file://").nth(1).unwrap().to_string();
        removable_media_disk_to_mirror(
            log,
            from,
            destination_registry.clone(),
            args.skip_blob_upload,
        )
        .await;
    }
}
