// use modules
use crate::additional::collector::*;
use crate::clusterresources::generate::*;
use crate::operator::collector::*;
use crate::release::collector::*;
use clap::Parser;
use custom_logger::*;
use mirror_copy::{ImplDownloadImageInterface, ImplUploadImageInterface};
use mirror_utils::fs_handler;
use std::collections::HashMap;
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
mod graphdata;
mod operator;
mod podman;
mod release;
mod removable_media;

// use local modules
use api::schema::*;
use archive::create::*;
use config::load::*;
use removable_media::collector::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let level = args.loglevel.unwrap().to_string();
    let arch = args.architecture.to_string();

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

    // multi archj support
    let mut vec_arch: Vec<String> = Vec::new();
    if arch == "all" {
        vec_arch.insert(0, "amd64".to_string());
        vec_arch.insert(0, "arm64".to_string());
        vec_arch.insert(0, "ppc64le".to_string());
        vec_arch.insert(0, "s390x".to_string());
        vec_arch.insert(0, "x86_64".to_string());
    } else {
        vec_arch = arch.split(",").map(|v| v.to_string()).collect();
        vec_arch.insert(0, "x86_64".to_string());
    }

    let mut mp = MirrorParameters {
        architectures: vec_arch.clone(),
        dir: "".to_string(),
        dry_run: args.dry_run,
        from: args.from.clone(),
        destination: args.destination.clone(),
        skip_blob_upload: args.skip_blob_upload,
        skip_manifest_check: args.skip_manifest_check.as_ref().unwrap().to_string(),
        tls_verify: args.tls_verify,
        verify_blobs: args.verify_blobs,
        generic_override: HashMap::new(),
        rebuild_catalogs: Some(true),
    };

    // initialize the client request interface
    let reg_con = ImplDownloadImageInterface {};

    // this is mirrorToDisk
    if mp.destination.contains("file://") {
        if args.config.is_none() {
            log.error("the --config flag and value is mandatory");
            process::exit(1);
        }

        log.debug(&format!(
            "image-mirror config file {} ",
            args.config.as_ref().unwrap()
        ));

        // Parse the config serde_yaml::ImageSetConfiguration.
        let config = load_config(args.config.as_ref().unwrap().to_string()).await;
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

        let destination = mp.destination.split("file://").nth(1).unwrap();
        let res_mm = fs_handler(
            format!("{}/{}", destination, "mirror-metadata".to_string()),
            "create_dir",
            None,
        )
        .await;
        if res_mm.is_err() {
            log.error(&format!("{}", res_mm.err().unwrap().to_string()));
        }
        let res_mp = fs_handler(
            format!("{}/{}", destination, "mappings".to_string()),
            "create_dir",
            None,
        )
        .await;
        if res_mp.is_err() {
            log.error(&format!("{}", res_mp.err().unwrap().to_string()));
        };
        let res_mp = fs_handler(
            format!("{}/{}", destination, "artifacts".to_string()),
            "create_dir",
            None,
        )
        .await;
        if res_mp.is_err() {
            log.error(&format!("{}", res_mp.err().unwrap().to_string()));
        };

        mp.dir = destination.to_string();

        // check for release images
        if isc_config_final.mirror.release.is_some() {
            let res = release_mirror_to_disk(
                reg_con.clone(),
                log,
                isc_config_final.mirror.release.unwrap(),
                mp.clone(),
            )
            .await;
            if res.is_err() {
                log.error(&format!(
                    "result from release collector {}",
                    res.err().unwrap()
                ));
                process::exit(1);
            }
        }
        // check for operators
        if isc_config_final.mirror.operators.is_some() {
            let res = operator_mirror_to_disk(
                reg_con.clone(),
                log,
                isc_config_final.mirror.operators.unwrap(),
                mp.clone(),
            )
            .await;

            if res.is_err() {
                log.error(&format!(
                    "result from operator collector {}",
                    res.err().unwrap()
                ));
                process::exit(1);
            }
        }
        // check for additional images
        if isc_config_final.mirror.additional_images.is_some() {
            let res = additional_mirror_to_disk(
                reg_con.clone(),
                log,
                isc_config_final.mirror.additional_images.unwrap(),
                mp.clone(),
            )
            .await;
            if res.is_err() {
                log.error(&format!(
                    "result from additional images collector {}",
                    res.err().unwrap()
                ));
            }
        }

        // finally create tar archive
        if !mp.dry_run {
            // archive_size set to 5G
            let mut archive_size = 1024 * 1024 * 1024 * 5;
            if isc_config_final.archive_size.is_some() {
                let size = isc_config_final.archive_size.unwrap();
                archive_size = 1024 * 1024 * 1024 * size;
            }
            log.info("creating tar files");
            vec_arch.insert(0, "all".to_string());
            let res = create_tar(log, destination.to_string(), archive_size, vec_arch).await;
            if res.is_err() {
                log.error(&format!("{}", res.err().unwrap()));
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
        let g_impl = ImplUploadImageInterface {};
        let from = args.from.split("file://").nth(1).unwrap().to_string();
        let res_rm = removable_media_disk_to_mirror(
            g_impl.clone(),
            log,
            from.clone(),
            destination_registry.clone(),
            mp.clone(),
        )
        .await;
        if res_rm.is_err() {
            log.error(&format!("{}", res_rm.err().unwrap()));
            process::exit(1);
        }

        // generate idms, itms and catalog source
        let gcr = GenerateClusterResources::new(from.clone());
        let res_un = gcr.untar_metadata(log).await;
        if res_un.is_err() {
            log.error(&format!(
                "untarring archive (metadata) {:#}",
                res_un.err().unwrap()
            ));
        }

        let gen_res = gcr
            .generate_idms_itms(log, from.clone(), destination_registry.clone())
            .await;
        if gen_res.is_err() {
            log.error(&format!("{}", gen_res.err().unwrap().to_string()));
        }

        let gen_res = gcr
            .generate_catalog_source(log, from.clone(), destination_registry)
            .await;
        if gen_res.is_err() {
            log.error(&format!("{}", gen_res.err().unwrap().to_string()));
        }
        let res_c = gcr.clean_up(mp.dir.clone()).await;
        if res_c.is_err() {
            log.error(&format!("{}", res_c.err().unwrap().to_string()));
        }
    }
}
