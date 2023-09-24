// use modules
use clap::Parser;
use std::fs;
use std::path::Path;
use tokio;

// define local modules
mod api;
mod auth;
mod config;
mod image;
mod log;
mod manifests;
mod operator;

// use local modules
use api::schema::*;
use auth::credentials::*;
use config::read::*;
use image::copy::*;
use log::logging::*;
use manifests::catalogs::*;
use operator::collector::*;

// main entry point (use async)
#[tokio::main]
async fn main() {
    let args = Cli::parse();
    let cfg = args.config.as_ref().unwrap().to_string();

    let log = &Logging {
        log_level: Level::DEBUG,
    };

    log.info(&format!("rust-image-mirror {} ", cfg));

    // Parse the config serde_yaml::FilterConfiguration.
    let config = load_config(cfg).unwrap();
    let isc_config = parse_yaml_config(config).unwrap();
    log.debug(&format!("{:#?}", isc_config.mirror.operators));

    // parse the config - iterate through each catalog
    let img_ref = parse_image_index(log, isc_config.mirror.operators);
    log.debug(&format!("Image refs {:#?}", img_ref));

    for ir in img_ref {
        let manifest_json = get_manifest_json_file(ir.name.clone(), ir.version.clone());
        let token = get_token(log, ir.registry.clone()).await;
        // use token to get manifest
        let manifest_url = get_image_manifest_url(ir.clone());
        let manifest = get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        // create the full path
        fs::write(manifest_json, manifest.clone()).expect("unable to write file");
        let res = parse_json_manifest(manifest).unwrap();
        let blobs_url = get_blobs_url(ir.clone());
        // use a concurrent process to get related blobs
        get_blobs(log, blobs_url, token.clone(), res.fs_layers.clone()).await;
        log.info("completed image index download");

        let working_dir_cache = get_cache_dir(ir.name.clone(), ir.version.clone());
        // create the cache directory
        fs::create_dir_all(&working_dir_cache).expect("unable to create directory");
        untar_layers(log, working_dir_cache.clone(), res.fs_layers).await;
        log.hi("completed untar of layers");

        // find the directory 'configs'
        let dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
        log.mid(&format!("full path for directory 'configs' {} ", &dir));

        let images = mirror_to_disk(log, dir, ir.packages.clone()).await;
        log.lo(&format!("related_images {:#?}", images));

        for img in images.iter() {
            // first check if the manifest exists
            let op_name = get_operator_name(img.image.clone());
            log.trace(&format!("operator name {:#?}", op_name));
            let op_dir = get_operator_manifest_json_dir(
                ir.name.clone(),
                ir.version.clone(),
                op_name.clone(),
            );
            log.debug(&format!("operator manifest path {:#?}", op_dir));
            //let file = op_dir.clone() + "/manifest.json";
            //if !Path::new(&file).exists() {
            let manifest_url = get_manifest_url(img.image.clone());
            log.trace(&format!("image url {:#?}", manifest_url));
            let manifest = get_manifest(manifest_url.clone(), token.clone())
                .await
                .unwrap();
            log.trace(&format!("manifest {:#?}", manifest));
            // check if the manifest is of type list
            let manifest_list = parse_json_manifestlist(manifest.clone());
            fs::create_dir_all(&op_dir).expect("unable to create operator manifest directory");
            let mut fslayers = Vec::new();
            if manifest_list.is_ok() {
                let ml = manifest_list.unwrap().clone();
                if ml.media_type == "application/vnd.docker.distribution.manifest.list.v2+json" {
                    fs::write(op_dir.clone() + "/manifest-list.json", manifest.clone())
                        .expect("unable to write file");
                    // look for the digest
                    // loop through each manifest
                    for mf in ml.manifests.iter() {
                        let sub_manifest_url = get_manifest_url_by_digest(
                            img.image.clone(),
                            mf.digest.clone().unwrap(),
                        );
                        let local_manifest = get_manifest(sub_manifest_url.clone(), token.clone())
                            .await
                            .unwrap();

                        fs::write(
                            op_dir.clone()
                                + "/manifest-"
                                + &mf.platform.clone().unwrap().architecture
                                + ".json",
                            local_manifest.clone(),
                        )
                        .expect("unable to write file");
                        // convert op_manifest.layer to FsLayer and add it to the collection
                        let op_manifest =
                            parse_json_manifest_operator(local_manifest.clone()).unwrap();
                        for layer in op_manifest.layers.unwrap().iter() {
                            let fslayer = FsLayer {
                                blob_sum: layer.digest.clone(),
                            };
                            fslayers.insert(0, fslayer);
                        }
                        // add configs
                        let cfg = FsLayer {
                            blob_sum: op_manifest.config.unwrap().digest,
                        };
                        fslayers.insert(0, cfg);
                    }
                }
            } else {
                fs::write(op_dir + "/manifest.json", manifest.clone())
                    .expect("unable to write file");
                // now download each related images blobs
                let op_manifest = parse_json_manifest_operator(manifest.clone()).unwrap();
                log.trace(&format!("op_manifest {:#?}", op_manifest));
                // convert op_manifest.layer to FsLayer
                for layer in op_manifest.layers.unwrap().iter() {
                    let fslayer = FsLayer {
                        blob_sum: layer.digest.clone(),
                    };
                    fslayers.insert(0, fslayer);
                }
                // add configs
                let cfg = FsLayer {
                    blob_sum: op_manifest.config.unwrap().digest,
                };
                fslayers.insert(0, cfg);
            }
            let op_url = get_blobs_url_by_string(img.image.clone());
            get_blobs(log, op_url, token.clone(), fslayers.clone()).await;
        }
    }
}

// utility functions - get_manifest_json
fn get_manifest_json_file(name: String, version: String) -> String {
    let mut file = String::from("working-dir/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/");
    file.push_str(&"manifest.json");
    file
}

// utility functions - get_operator_manifest_json_dir
fn get_operator_manifest_json_dir(name: String, version: String, operator: String) -> String {
    let mut file = String::from("working-dir/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/operators/");
    file.push_str(&operator);
    file
}

// get_cache_dir
fn get_cache_dir(name: String, version: String) -> String {
    let mut file = String::from("working-dir/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/");
    file.push_str(&"cache");
    file
}
