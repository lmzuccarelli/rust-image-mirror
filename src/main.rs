// use modules
use crate::additional::collector::*;
use crate::clusterresources::generate::*;
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
mod batch;
mod catalog;
mod clusterresources;
mod config;
mod error;
mod graphdata;
mod image;
mod operator;
mod podman;
mod release;
mod removable_media;

// use local modules
use api::schema::*;
//use clusterresources::*;
use archive::create::*;
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
    let config = load_config(cfg);
    if config.is_err() {
        log.error(&format!("{:#}", config.err().unwrap().to_string()));
        process::exit(1);
    }
    let isc_config = parse_yaml_config(config.unwrap());
    if isc_config.is_err() {
        log.error(&format!("{:#}", isc_config.err().unwrap().to_string()));
        process::exit(1);
    }

    let isc_config_final = isc_config.unwrap();

    log.debug(&format!(
        "image set config releases {:#?}",
        isc_config_final.mirror.release
    ));

    log.debug(&format!(
        "image set config operators {:#?}",
        isc_config_final.mirror.operators
    ));

    log.debug(&format!(
        "image set config additional images {:#?}",
        isc_config_final.mirror.additional_images
    ));

    // initialize the client request interface
    let reg_con = ImplRegistryInterface {};

    // this is mirrorToDisk
    if args.destination.contains("file://") {
        let destination = args.destination.split("file://").nth(1).unwrap();
        log.info(&format!("destination {}", destination));
        fs::create_dir_all(&format!(
            "{}/{}",
            destination,
            "mirror-metadata".to_string()
        ))
        .expect("should create manifests directory");

        fs::create_dir_all(&format!("{}/{}", destination, "mappings".to_string()))
            .expect("should create mappings directory");

        // check for release images
        let skip_manifest_check = skip_manifests == "release" || skip_manifests == "all";
        if isc_config_final.mirror.release.is_some() {
            let res = release_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config_final.mirror.release.unwrap(),
            )
            .await;
            if res.is_err() {
                log.error(&format!("{}", res.err().unwrap()));
                process::exit(1);
            }
        }
        // check for operators
        let skip_manifest_check = skip_manifests == "operators" || skip_manifests == "all";
        if isc_config_final.mirror.operators.is_some() {
            let res = operator_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config_final.mirror.operators.unwrap(),
            )
            .await;
            if res.is_err() {
                log.error(&format!("{}", res.err().unwrap()));
            }
        }
        // check for additional images
        let skip_manifest_check = skip_manifests == "additional" || skip_manifests == "all";
        if isc_config_final.mirror.additional_images.is_some() {
            let res = additional_mirror_to_disk(
                reg_con.clone(),
                log,
                destination.to_string(),
                skip_manifest_check,
                dry_run,
                isc_config_final.mirror.additional_images.unwrap(),
            )
            .await;
            if res.is_err() {
                log.error(&format!("{}", res.err().unwrap()));
            }
        }

        // finally create tar archive
        if !dry_run && !skip_manifest_check {
            log.info("creating tar files");
            let res = create_tar(log, destination.to_string());
            if res.is_err() {
                log.error(&format!("error creating tar {}", res.err().unwrap()));
                process::exit(1);
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
                log.error("from director with protocol must have file::// prefix");
                process::exit(exitcode::USAGE);
            }
        }
        let from = args.from.split("file://").nth(1).unwrap().to_string();
        let res = removable_media_disk_to_mirror(
            log,
            from.clone(),
            destination_registry.clone(),
            args.skip_blob_upload,
            true,
        )
        .await;
        if res.is_err() {
            log.error(&format!("error creating tar {}", res.err().unwrap()));
            process::exit(1);
        }

        // generate idms, itms and catalog source
        let gcr = GenerateClusterResources::new(from.clone());
        let res = gcr.untar_metadata(log);
        if res.is_err() {
            log.error(&format!(
                "untarring archive (metadata) {:#}",
                res.err().unwrap()
            ));
        }

        let gen_res = gcr.generate_idms_itms(log, from.clone(), destination_registry.clone());
        if gen_res.is_err() {
            log.error(&format!("{:#}", gen_res.err().unwrap()));
        }

        let gen_res = gcr.generate_catalog_source(log, from.clone(), destination_registry);
        if gen_res.is_err() {
            log.error(&format!("{:#}", gen_res.err().unwrap()));
        }

        let res = gcr.clean_up();
        if res.is_err() {
            log.error(&format!("{:#}", res.err().unwrap()));
        }
    }
}
