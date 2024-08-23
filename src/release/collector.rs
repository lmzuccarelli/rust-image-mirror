use crate::api::schema::MirrorImageInfo;
use crate::batch::worker::*;
use crate::config::load::*;
use crate::error::handler::MirrorError;
use crate::graphdata::process::GraphDataInterface;
use crate::graphdata::process::ImplGraphDataInterface;
use crate::image::utils::*;
use chrono::{DateTime, Local};
use custom_logger::*;
use hex::encode;
use mirror_auth::get_token;
use mirror_catalog_index::*;
use mirror_copy::*;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process;
use std::usize;
use walkdir::WalkDir;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReleaseSchema {
    #[serde(rename = "spec")]
    pub spec: Spec,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    #[serde(rename = "metadata")]
    pub metadata: MetaData,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Spec {
    #[serde(rename = "lookupPolicy")]
    pub lookup: LookupPolicy,
    #[serde(rename = "tags")]
    pub tags: Vec<Tags>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LookupPolicy {
    #[serde(rename = "local")]
    pub local: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Tags {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "from")]
    pub from: From,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct From {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "kind")]
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MetaData {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "creationTimestamp")]
    pub creation: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReleaseImageInfo {
    pub file: String,
    pub original_ref: String,
}

// collect all operator images
pub async fn release_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    skip_manifests_check: bool,
    dry_run: bool,
    releases: Release,
    verify_blobs: bool,
) -> Result<(), MirrorError> {
    log.hi("release collector mode: mirror-to-disk");

    // set up dir to store all manifests
    fs_handler(
        format!("{}/{}", dir.clone(), "/manifests/release".to_string()),
        "create_dir",
        None,
    )?;

    let mut vec_process_manifests: Vec<ReleaseImageInfo> = Vec::new();
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();
    let mut fslayers: HashMap<String, Vec<FsLayer>> = HashMap::new();

    // parse the config
    for release in releases.images.iter() {
        // parse image index
        let index_image_ref = convert_release_image_index(log, release.name.clone());
        log.debug(&format!("image refs {:#?}", index_image_ref.clone()));
        let token = get_token(log, index_image_ref.clone().registry).await;
        let mut manifest: String = String::new();
        let release_manifest_file = format!(
            "{}/{}/{}/manifest.json",
            dir.clone(),
            "ocp-release",
            index_image_ref.version
        );

        if !skip_manifests_check {
            if token.is_err() {
                // if token is not found or expired
                // there is no use continuing
                log.error(&format!("{:#?}", token.err().unwrap()));
                process::exit(1);
            }
            // construct manifest api call
            let manifest_url = &format!(
                "https://{}/v2/{}/{}/manifests/{}",
                index_image_ref.registry,
                index_image_ref.namespace,
                index_image_ref.name,
                index_image_ref.version
            );
            log.mid(&format!("api call for manifest {}", release.name.clone()));

            let res = reg_con
                .get_manifest(manifest_url.clone(), token.as_ref().unwrap().to_string())
                .await;
            if res.is_ok() {
                let manifest_mem = res.unwrap();
                let mut exists = Path::new(&release_manifest_file.clone()).exists();
                if exists {
                    let res_data = fs::read_to_string(release_manifest_file.clone());
                    if res_data.is_ok() {
                        if manifest_mem != res_data.unwrap() {
                            fs_handler(
                                release_manifest_file.clone(),
                                "write",
                                Some(manifest_mem.clone()),
                            )?;
                            exists = false;
                        }
                    } else {
                        exists = false
                    }
                }
                if !exists {
                    manifest = manifest_mem.clone();
                    let release_image_info = ReleaseImageInfo {
                        file: release_manifest_file.clone(),
                        original_ref: release.name.clone(),
                    };
                    vec_process_manifests.insert(0, release_image_info);
                }
            } else {
                let err = MirrorError::new(&format!(
                    "release api call  {}",
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            // read from disk
            let res_data = fs::read_to_string(release_manifest_file.clone());
            if res_data.is_ok() {
                manifest = res_data.unwrap();
            } else {
                let err = MirrorError::new(&format!(
                    "release reading manifest  {}",
                    res_data.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }

        // multi arch
        // in the cli ensure that only multi is entered
        // and no other platform architecture
        if release.name.clone().contains("multi") {
            let manifest_list = parse_json_manifestlist(manifest.clone());
            if manifest_list.is_ok() {
                for mfst in manifest_list.unwrap().manifests.iter() {
                    // contruct api call for manifests
                    let manifest_url = &format!(
                        "https://{}/v2/{}/{}/manifests/{}",
                        index_image_ref.clone().registry,
                        index_image_ref.clone().namespace,
                        index_image_ref.clone().name,
                        mfst.digest.as_ref().unwrap()
                    );
                    log.info(&format!(
                        "checking multi arch manifest for {}",
                        release.name.clone() + "/" + mfst.digest.as_ref().unwrap()
                    ));

                    let original_ref = format!(
                        "{}-{}",
                        release.name.clone().split("-multi").nth(0).unwrap(),
                        mfst.platform.as_ref().unwrap().architecture
                    );

                    let inner_manifest = reg_con
                        .get_manifest(manifest_url.clone(), token.as_ref().unwrap().to_string())
                        .await;

                    if inner_manifest.is_ok() {
                        // create the directory to store manifests in
                        let manifest_json_dir = &format!(
                            "{}/{}/{}-{}",
                            dir.clone(),
                            "ocp-release",
                            index_image_ref.clone().version.split("-").nth(0).unwrap(),
                            mfst.platform.as_ref().unwrap().architecture,
                        );
                        log.info(&format!("manifest_json_dir {}", manifest_json_dir.clone()));
                        fs_handler(manifest_json_dir.to_string(), "create_dir", None)?;
                        let mfst_file = format!("{}/manifest.json", manifest_json_dir);
                        // check if it exists first
                        let exists = Path::new(&mfst_file).exists();
                        if exists {
                            let msft_on_disk = fs::read_to_string(mfst_file.clone());
                            if msft_on_disk.is_ok() {
                                if msft_on_disk.unwrap()
                                    != inner_manifest.as_ref().unwrap().to_string()
                                {
                                    fs_handler(
                                        mfst_file.clone(),
                                        "write",
                                        Some(inner_manifest.unwrap().to_string()),
                                    )?;
                                    let release_image_info = ReleaseImageInfo {
                                        file: mfst_file.clone(),
                                        original_ref: original_ref.clone(),
                                    };
                                    vec_process_manifests.insert(0, release_image_info);
                                }
                            }
                        } else {
                            fs_handler(
                                mfst_file.clone().to_string(),
                                "write",
                                Some(inner_manifest.unwrap().to_string()),
                            )?;
                            let release_image_info = ReleaseImageInfo {
                                file: mfst_file.clone(),
                                original_ref: original_ref.clone(),
                            };
                            vec_process_manifests.insert(0, release_image_info);
                        }
                    } else {
                        let err = MirrorError::new(&format!(
                            "release servere multi arch  {}",
                            inner_manifest.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
            }
        }

        if vec_process_manifests.clone().len() == 0 {
            log.info("no change detected in manifest files")
        }

        for mf in vec_process_manifests.clone().iter() {
            log.info("changed detected in manifest file/s");
            log.info(&format!("processing {} ", mf.file));
            log.debug(&format!("original ref {} ", mf.original_ref));
            let manifest_on_disk = fs::read_to_string(&mf.file);
            let mut vec_fslayer: Vec<FsLayer> = Vec::new();

            let image_ref = convert_release_image_index(log, mf.clone().original_ref);

            let blobs_url = &format!(
                "https://{}/v2/{}/{}/blobs/",
                image_ref.registry, image_ref.namespace, image_ref.name
            );
            let blobs_dir = &format!("{}/{}/", dir.clone(), "blobs-store",);
            log.info(&format!("blobs_dir {}", blobs_dir.clone()));

            if manifest_on_disk.is_ok() {
                let data = manifest_on_disk.unwrap();
                let parsed_manifest = parse_json_manifest_operator(data.clone().to_string());
                if parsed_manifest.is_ok() {
                    // not oci format
                    let version = parsed_manifest.as_ref().unwrap().schema_version.unwrap();
                    if version == 1 {
                        let parsed_manifest = parse_json_manifest(data.clone().to_string());
                        if parsed_manifest.is_ok() {
                            let v1_mnfst = parsed_manifest.unwrap();
                            let response = reg_con
                                .get_blobs(
                                    log,
                                    blobs_dir.clone(),
                                    blobs_url.to_string(),
                                    token.as_ref().unwrap().to_string(),
                                    v1_mnfst.fs_layers.clone(),
                                )
                                .await;
                            if response.is_err() {
                                let err = MirrorError::new(&format!(
                                    "manifest api call {}",
                                    response.err().unwrap().to_string().to_lowercase()
                                ));
                                return Err(err);
                            }
                            vec_fslayer.append(&mut v1_mnfst.fs_layers.clone());
                            log.info("completed release image index (v1) download");
                        }
                    } else {
                        let layers = parsed_manifest.as_ref().unwrap();
                        // ignore config layer
                        for layer in layers.clone().layers.unwrap().iter() {
                            let fslayer = FsLayer {
                                blob_sum: layer.digest.clone(),
                                original_ref: Some(mf.clone().original_ref),
                                size: Some(layer.size),
                                number: None,
                            };
                            vec_fslayer.insert(0, fslayer);
                        }
                        let response = reg_con
                            .get_blobs(
                                log,
                                blobs_dir.clone(),
                                blobs_url.to_string(),
                                token.as_ref().unwrap().to_string(),
                                vec_fslayer.clone(),
                            )
                            .await;

                        if response.is_err() {
                            let err = MirrorError::new(&format!(
                                "get_blob api call {}",
                                response.err().unwrap().to_string().to_lowercase()
                            ));
                            return Err(err);
                        }
                        log.info("completed release image index (v2) download");
                    }
                }
            } else {
                let err = MirrorError::new(&format!(
                    "reading manifest {}",
                    manifest_on_disk.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }

            let working_dir_cache = &format!(
                "{}/{}/{}/cache",
                dir.clone(),
                image_ref.name.clone(),
                image_ref.version.clone(),
            );
            log.info(&format!("working_dir_cache {}", working_dir_cache));

            let cache_exists = Path::new(&working_dir_cache).exists();
            if cache_exists {
                rm_rf::remove(&working_dir_cache).expect("should delete current untarred cache");
            }

            let mut builder = DirBuilder::new();
            builder.mode(0o777);
            builder
                .create(&working_dir_cache)
                .expect("unable to create directory");

            untar_layers(
                log,
                blobs_dir.clone(),
                working_dir_cache.clone(),
                vec_fslayer.clone(),
            )
            .await;
            log.hi("completed untar of layers");
        }

        // find the directory 'release-manifests'
        // use the release image version (before the "-") to match
        let release_version = release.name.split(":").nth(1).unwrap();
        let version = release_version.split("-").nth(0).unwrap();
        let arch = release_version.split("-").nth(1).unwrap();

        let base_dir = format!("{}/{}", dir.clone(), "ocp-release");

        let dt = Local::now();
        let naive_utc = dt.naive_utc();
        let offset = dt.offset().clone();
        let dt_new = DateTime::<Local>::from_naive_utc_and_offset(naive_utc, offset);
        let dt_formated = dt_new.format("%Y-%m-%d %H:%M:%S%.3f");
        let mut vec_common_blobs: Vec<String> = Vec::new();
        let mut vec_flayer: Vec<FsLayer> = Vec::new();

        for e in WalkDir::new(base_dir.clone().to_string()) {
            let obj = e.unwrap();
            if obj.path().is_file() {
                let manifest_file = obj.path().to_string_lossy();
                if manifest_file.contains(version) && manifest_file.contains("image-references") {
                    log.info(&format!(
                        "processing release-references {:#?} ",
                        manifest_file.clone(),
                    ));
                    let res_manifest_in_mem =
                        parse_json_release_imagereference(manifest_file.clone().to_string());
                    if res_manifest_in_mem.is_ok() {
                        let rm = res_manifest_in_mem.unwrap();
                        for img in rm.clone().spec.tags.iter() {
                            let image_ref = parse_image(log, img.clone().from.name);
                            if !skip_manifests_check && !dry_run {
                                let manifest_url = &format!(
                                    "https://{}/v2/{}/{}/manifests/{}",
                                    image_ref.registry,
                                    image_ref.namespace,
                                    image_ref.name,
                                    image_ref.version,
                                );
                                log.trace(&format!("manifest url {:#?}", manifest_url.clone()));
                                // use the RegistryInterface to make the call
                                let res_manifest = reg_con
                                    .get_manifest(
                                        manifest_url.clone(),
                                        token.as_ref().unwrap().to_string(),
                                    )
                                    .await;
                                log.mid(&format!("api call for manifest {:#?}", img.name));

                                if res_manifest.is_ok() {
                                    let manifest = res_manifest.unwrap();
                                    let digest = get_sha_from_contents(manifest.clone().as_bytes());
                                    if digest != image_ref.version.split(":").nth(1).unwrap() {
                                        log.warn(&format!(
                                            "digest and file sha does not match {} : {}",
                                            img.name, image_ref.version
                                        ));
                                    }
                                    let f = &format!(
                                        "{}/manifests/release/{}-{}.json",
                                        dir.clone(),
                                        image_ref.version,
                                        arch
                                    );
                                    let mut exists = true;
                                    let manifest_on_disk = fs::read_to_string(f);
                                    if manifest_on_disk.is_ok() {
                                        if manifest_on_disk.unwrap() != manifest {
                                            exists = false;
                                        }
                                    } else {
                                        exists = false;
                                    }
                                    if !exists {
                                        fs_handler(
                                            f.to_string(),
                                            "write",
                                            Some(manifest.clone().to_string()),
                                        )?;
                                    }
                                } else {
                                    let err = MirrorError::new(&format!(
                                        "manifest api call {}",
                                        res_manifest.err().unwrap().to_string().to_lowercase()
                                    ));
                                    return Err(err);
                                }
                            }

                            let mnfst_on_disk = format!(
                                "{}/manifests/release/{}-{}.json",
                                dir.clone(),
                                image_ref.version,
                                arch
                            );

                            // it may seem ridiculous to write then read from disk
                            // this was done intentionally as it ensures that our manifest
                            // file is correct and parsable
                            let md = fs::read_to_string(mnfst_on_disk.clone());
                            if md.is_ok() {
                                log.info(&format!("checking manifest {}", img.name));
                                log.debug(&format!("sha {} ", image_ref.version));
                                let op_manifest =
                                    parse_json_manifest_operator(md.unwrap()).unwrap();

                                let op_url = format!(
                                    "https://{}/v2/{}/{}/blobs/",
                                    image_ref.registry, image_ref.namespace, image_ref.name,
                                );

                                for layer in op_manifest.layers.unwrap().iter() {
                                    // check for duplicates
                                    if vec_common_blobs.contains(&layer.digest) {
                                        continue;
                                    }
                                    vec_common_blobs.push(layer.digest.clone());
                                    // convert op_manifest.layer to FsLayer
                                    let fslayer = FsLayer {
                                        blob_sum: layer.digest.clone(),
                                        original_ref: Some(img.from.name.clone()),
                                        size: Some(layer.size),
                                        number: None,
                                    };
                                    vec_flayer.insert(0, fslayer);
                                }
                                // add configs
                                let config = op_manifest.config.unwrap();
                                let cfg = FsLayer {
                                    blob_sum: config.digest,
                                    original_ref: Some(img.from.name.clone()),
                                    size: Some(config.size),
                                    number: None,
                                };
                                vec_flayer.insert(0, cfg);
                                // finally add the fslayers to the hashmap
                                fslayers.insert(op_url.clone(), vec_flayer.clone());
                            } else {
                                let err = MirrorError::new(&format!(
                                    "reading manifest form disk {}",
                                    md.err().unwrap().to_string().to_lowercase()
                                ));
                                return Err(err);
                            }

                            let mii = MirrorImageInfo {
                                reference: img.clone().from.name.clone(),
                                name: img.name.clone(),
                                arch: arch.to_string(),
                                namespace: image_ref.namespace + &"/" + &image_ref.name,
                                digest: image_ref.version,
                                manifest_type: "manifest".to_string(),
                                tag: None,
                                created: dt_formated.to_string(),
                                mirror_type: "release".to_string(),
                                bundle: None,
                            };
                            image_ref_tracker.insert(0, mii.clone());
                        }
                    } else {
                        let err = MirrorError::new(&format!(
                            "parsing release reference {}",
                            res_manifest_in_mem
                                .err()
                                .unwrap()
                                .to_string()
                                .to_lowercase()
                        ));
                        return Err(err);
                    }
                }
            }
        }
    }

    if releases.graph.is_some() && !dry_run {
        if releases.graph.unwrap().contains("true") {
            log.mid("build graph data");
            let g_impl = ImplGraphDataInterface {};
            let res = g_impl.build_graph_image(log, dir.clone()).await;
            if res.is_ok() {
                let p_fbi = process_fb_image(
                    dir.clone(),
                    "graph-image".to_string(),
                    "openshift".to_string(),
                    "latest".to_string(),
                    "release".to_string(),
                );
                if p_fbi.is_ok() {
                    image_ref_tracker.insert(0, p_fbi.unwrap());
                } else {
                    let err = MirrorError::new(&format!(
                        "process fb image {}",
                        p_fbi.err().unwrap().to_string().to_lowercase()
                    ));
                    return Err(err);
                }
            } else {
                let err = MirrorError::new(&format!(
                    "reading tar file {}",
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
            g_impl.build_image_cleanup().await;
        }
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs_handler(
        dir.clone() + &"/mirror-metadata/release-image-reference.json",
        "write",
        Some(serialized_manifest),
    )?;

    if dry_run {
        let mut buf = String::from("");
        let data =
            fs::read_to_string(dir.clone() + &"/mirror-metadata/release-image-reference.json");
        if data.is_ok() {
            let air = parse_json_metadata(data.unwrap());
            if air.is_ok() {
                for mii in air.unwrap().iter() {
                    if mii.arch == "amd64" || mii.arch == "x86_64" {
                        let src = &format!("{}{}", "docker://", mii.reference);
                        let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                        buf = buf + &format!("{}={}\n", src, dest);
                    }
                }
                fs_handler(
                    dir.clone() + "/mappings/release-mapping.txt",
                    "write",
                    Some(buf),
                )?;
                log.mid(&format!(
                    "created release mapping file in folder {}",
                    dir.clone() + &"/mappings/",
                ));
            } else {
                let err = MirrorError::new(&format!(
                    "parsing release metadata {}",
                    air.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            let err = MirrorError::new(&format!(
                "reading release metadata file {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    } else {
        let map = remove_duplicates(dir.clone(), fslayers);
        let res = execute_batch(log, dir.clone(), verify_blobs, map).await;
        if res.is_err() {
            return Err(res.err().unwrap());
        }
    }
    Ok(())
}

// utility functions section

// get_sha_from_contents
pub fn get_sha_from_contents(manifest_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest_bytes);
    let hash_bytes = hasher.finalize();
    encode(hash_bytes)
}

pub fn parse_json_release_imagereference(
    file: String,
) -> Result<ReleaseSchema, Box<dyn std::error::Error>> {
    let data = fs::read_to_string(&file)
        .expect("should read release-manifests/image-references json file");
    // Parse the string of data into ReleaseSchema
    let root: ReleaseSchema = serde_json::from_str(&data)?;
    Ok(root)
}

// parse_release_image_index - best attempt to parse image index and return catalog reference
pub fn convert_release_image_index(log: &Logging, release: String) -> ImageReference {
    let hld = &mut release.split("/");
    let reg = hld.nth(0).unwrap();
    let ns = hld.nth(0).unwrap();
    let mut index = hld.nth(0).unwrap().split(":");
    let name = index.nth(0).unwrap();
    let ver = index.nth(0).unwrap();
    let ir = ImageReference {
        registry: reg.to_string(),
        namespace: ns.to_string(),
        name: name.to_string(),
        version: ver.to_string(),
    };
    log.trace(&format!("image reference {:#?}", ir));
    ir
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;

    #[test]
    fn mirror_to_disk_pass() {
        let _log = &Logging {
            log_level: Level::TRACE,
        };

        // we set up a mock server for the auth-credentials
        let mut server = mockito::Server::new();
        let _url = server.url();

        // Create a mock
        server
            .mock("GET", "/auth")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"token\": \"test\",
                    \"access_token\": \"aebcdef1234567890\",
                    \"expires_in\":300,
                    \"issued_at\":\"2023-10-20T13:23:31Z\"
                }",
            )
            .create();

        //#[derive(Clone)]
        //struct Fake {}
    }
}
