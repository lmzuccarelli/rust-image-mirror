use crate::api::schema::*;
use crate::auth::credentials::*;
use crate::batch::copy::*;
use crate::index::resolve::*;
use crate::log::logging::*;
use crate::manifests::catalogs::*;

use std::fs;

// collect all operator images
pub async fn mirror_to_disk(log: &Logging, dir: String, operator: Vec<Operator>) {
    log.info("operator collector mode: mirrorToDisk");

    // parse the config - iterate through each catalog
    let img_ref = parse_image_index(log, operator);
    log.debug(&format!("Image refs {:#?}", img_ref));

    for ir in img_ref {
        let manifest_json = get_manifest_json_file(
            // ./working-dir
            dir.clone(),
            ir.name.clone(),
            ir.version.clone(),
        );
        log.trace(&format!("manifest json file {}", manifest_json));
        let token = get_token(log, ir.registry.clone()).await;
        // use token to get manifest
        let manifest_url = get_image_manifest_url(ir.clone());
        let manifest = get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        // create the full path
        // TODO:
        // fs::create_dir_all(manifest_json.clone()).expect("unable to create directory");
        fs::write(manifest_json, manifest.clone()).expect("unable to write file");
        let res = parse_json_manifest(manifest).unwrap();
        let blobs_url = get_blobs_url(ir.clone());
        // use a concurrent process to get related blobs
        let sub_dir = dir.clone() + "/blobs-store/";
        get_blobs(
            log,
            sub_dir.clone(),
            blobs_url,
            token.clone(),
            res.fs_layers.clone(),
        )
        .await;
        log.info("completed image index download");

        let working_dir_cache = get_cache_dir(dir.clone(), ir.name.clone(), ir.version.clone());
        // create the cache directory
        fs::create_dir_all(&working_dir_cache).expect("unable to create directory");
        untar_layers(
            log,
            sub_dir.clone(),
            working_dir_cache.clone(),
            res.fs_layers,
        )
        .await;
        log.hi("completed untar of layers");

        // find the directory 'configs'
        // TODO if new blobs are downloaded the config dir could be in another blob
        let config_dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
        log.mid(&format!(
            "full path for directory 'configs' {} ",
            &config_dir
        ));

        let wrappers = get_related_images_from_catalog(log, config_dir, ir.packages.clone());
        // log.trace(&format!("images from catalog for {:#?}", ir.packages));

        // iterate through all the related images
        for wrapper in wrappers.iter() {
            for imgs in wrapper.images.iter() {
                // first check if the manifest exists
                let op_name = get_operator_name(imgs.image.clone());
                log.trace(&format!("operator name {:#?}", op_name));
                let op_dir = get_operator_manifest_json_dir(
                    dir.clone(),
                    ir.name.clone(),
                    ir.version.clone(),
                    op_name.clone(),
                    wrapper.channel.clone(),
                );
                log.debug(&format!("operator manifest path {:#?}", op_dir));
                //let file = op_dir.clone() + "/manifest.json";
                //if !Path::new(&file).exists() {
                let manifest_url = get_manifest_url(imgs.image.clone());
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
                    if ml.media_type == "application/vnd.docker.distribution.manifest.list.v2+json"
                    {
                        fs::write(op_dir.clone() + "/manifest-list.json", manifest.clone())
                            .expect("unable to write file");
                        // look for the digest
                        // loop through each manifest
                        for mf in ml.manifests.iter() {
                            let sub_manifest_url = get_manifest_url_by_digest(
                                imgs.image.clone(),
                                mf.digest.clone().unwrap(),
                            );
                            let local_manifest =
                                get_manifest(sub_manifest_url.clone(), token.clone())
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
                                    original_ref: Some(imgs.image.clone()),
                                    result: Some(String::from("")),
                                };
                                fslayers.insert(0, fslayer);
                            }
                            // add configs
                            let cfg = FsLayer {
                                blob_sum: op_manifest.config.unwrap().digest,
                                original_ref: Some(imgs.image.clone()),
                                result: Some(String::from("")),
                            };
                            fslayers.insert(0, cfg);
                        }
                    }
                } else {
                    fs::write(op_dir.clone() + "/manifest.json", manifest.clone())
                        .expect("unable to write file");
                    // now download each related images blobs
                    log.debug(&format!("manifest dir {:#?}", op_dir));
                    let op_manifest = parse_json_manifest_operator(manifest.clone()).unwrap();
                    log.trace(&format!("op_manifest {:#?}", op_manifest));
                    // convert op_manifest.layer to FsLayer
                    for layer in op_manifest.layers.unwrap().iter() {
                        let fslayer = FsLayer {
                            blob_sum: layer.digest.clone(),
                            original_ref: Some(imgs.image.clone()),
                            result: Some(String::from("")),
                        };
                        fslayers.insert(0, fslayer);
                    }
                    // add configs
                    let cfg = FsLayer {
                        blob_sum: op_manifest.config.unwrap().digest,
                        original_ref: Some(imgs.image.clone()),
                        result: Some(String::from("")),
                    };
                    fslayers.insert(0, cfg);
                }
                let op_url = get_blobs_url_by_string(imgs.image.clone());
                get_blobs(
                    log,
                    sub_dir.clone(),
                    op_url,
                    token.clone(),
                    fslayers.clone(),
                )
                .await;
            }
        }
    }
}

fn get_related_images_from_catalog(
    log: &Logging,
    dir: String,
    packages: Vec<Package>,
) -> Vec<RelatedImageWrapper> {
    let mut bundle_name = String::from("");
    let mut related_image_wrapper = Vec::new();

    for pkg in packages {
        let dc_json = read_operator_catalog(dir.to_string() + &"/".to_string() + &pkg.name)
            .unwrap()
            .clone();
        let dc: Vec<DeclarativeConfig> = serde_json::from_value(dc_json.clone()).unwrap();

        log.lo(&format!(
            "default channel {:?} for operator {} ",
            dc[0].default_channel, pkg.name
        ));

        // first check if channels are valid
        if pkg.channels.is_some() {
            // iterate through each channel
            for chn in pkg.channels.unwrap().iter() {
                for obj in dc.iter() {
                    if obj.schema == "olm.channel" {
                        log.trace(&format!("channels compare {:#?} {:#?}", chn.name, obj.name));
                        if chn.name == obj.name {
                            // get the entries object[0] which is the bundle we are after
                            // TODO: could be more than one entry
                            let entries: Vec<ChannelEntry> = match obj.entries.clone() {
                                Some(val) => val,
                                None => vec![],
                            };
                            bundle_name = entries[0].name.clone();
                        }
                    }
                    if obj.schema == "olm.bundle" {
                        if bundle_name == obj.name {
                            log.trace(&format!("bundle {:#?} {}", obj.related_images, obj.name));
                            let wrapper = RelatedImageWrapper {
                                images: obj.related_images.clone().unwrap(),
                                channel: chn.name.clone(),
                            };
                            related_image_wrapper.insert(0, wrapper);
                        }
                    }
                }
            }
        } else {
            // case when we don't have channels in the imageset config
            // we look for default channel
            for obj in dc.iter() {
                if obj.schema == "olm.bundle" {
                    if bundle_name == obj.name {
                        log.trace(&format!("bundle {:#?} {}", obj.related_images, obj.name));
                        let wrapper = RelatedImageWrapper {
                            images: obj.related_images.clone().unwrap(),
                            channel: dc[0].default_channel.clone().unwrap(),
                        };
                        related_image_wrapper.insert(0, wrapper);
                    }
                }
                if obj.schema == "olm.channel" {
                    if obj.name == dc[0].default_channel.clone().unwrap() {
                        log.trace(&format!("channel info {:#?} {}", obj.entries, obj.name));
                        let entries: Vec<ChannelEntry> = match obj.entries.clone() {
                            Some(val) => val,
                            None => vec![],
                        };
                        bundle_name = entries[0].name.clone();
                    }
                }
            }
        }
    }
    related_image_wrapper
}

// construct the operator namespace and name
fn get_operator_name(img: String) -> String {
    let mut parts = img.split("/");
    let _ = parts.nth(0).unwrap();
    let ns = parts.nth(0).unwrap();
    let name = parts.nth(0).unwrap();
    let mut op_name = name.split("@");
    ns.to_string() + "/" + &op_name.nth(0).unwrap().to_owned()
}

// utility functions - get_operator_manifest_json_dir
fn get_operator_manifest_json_dir(
    dir: String,
    name: String,
    version: String,
    operator: String,
    channel: String,
) -> String {
    // ./working-dir
    let mut file = dir.clone();
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/operators/");
    file.push_str(&operator);
    file.push_str(&"/");
    file.push_str(&channel);
    file
}
