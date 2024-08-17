use crate::api::schema::MirrorImageInfo;
use crate::config::load::*;
use crate::image::utils::{parse_image, parse_json_metadata};
use custom_logger::*;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use hex::encode;
use mirror_auth::*;
use mirror_catalog::*;
use mirror_catalog_index::*;
use mirror_copy::*;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ManifestList {
    #[serde(rename = "manifests")]
    pub manifests: Vec<Manifest>,

    #[serde(rename = "mediaType")]
    pub media_type: String,
}

// used to add path and arch (platform) info for mirroring
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MirrorManifest {
    pub registry: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub component: String,
    pub channel: String,
    pub sub_component: String,
    pub manifest_file: String,
}

// collect all operator images
pub async fn operator_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    skip_manifests_check: bool,
    dry_run: bool,
    operators: Vec<Operator>,
) {
    log.hi("operator collector mode: mirror-to-disk");

    // set up dir to store all manifests
    fs::create_dir_all(&format!(
        "{}/{}",
        dir.clone(),
        "manifests/operator".to_string()
    ))
    .expect("should create manifests directory");

    // parse the config - iterate through each catalog
    let img_ref = parse_index(log, operators.clone());
    log.info(&format!("image refs {:#?}", img_ref));
    let mut futs = FuturesUnordered::new();
    let batch_size = 8;
    let blobs_dir = dir.clone() + "/blobs-store/";
    let mut image_vec: Vec<String> = Vec::new();
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();

    // get all relevant catalogs in config
    // download manifests and blobs if changed
    // untar and set /configs directory
    for ir in img_ref.iter() {
        let manifest_json = format!(
            "{}/{}/{}/{:#?}/manifest.json",
            dir.clone(),
            ir.name.clone(),
            ir.version.clone(),
            Some("amd64".to_string()),
        );

        log.trace(&format!("manifest json file {}", manifest_json));
        // use token to get manifest
        let token = get_token(log, ir.registry.clone()).await;
        if token.is_err() {
            log.error(&format!("{:#?}", token.err().unwrap()));
            process::exit(1)
        }

        // construct manifest api url
        let manifest_url = &format!(
            "https://{}/v2/{}/{}/manifests/{}",
            ir.registry, ir.namespace, ir.name, ir.version
        );

        log.ex(&format!(
            "checking manifest for {:#?}",
            format!(
                "{}/{}/{}/{}",
                ir.registry, ir.namespace, ir.name, ir.version
            )
        ));

        let manifest = reg_con
            .get_manifest(manifest_url.clone(), token.as_ref().unwrap().to_string())
            .await;

        if manifest.is_ok() {
            let local_manifest = manifest.unwrap().clone();
            log.trace(&format!("manifest {:#}", local_manifest.clone()));
            let manifest_list = parse_json_manifestlist(local_manifest.clone());
            if manifest_list.is_ok() {
                for m in manifest_list.unwrap().manifests.iter() {
                    let arch = m.platform.as_ref().unwrap().architecture.to_string();
                    let manifest_json = format!(
                        "{}/{}/{}/{}/manifest.json",
                        dir.clone(),
                        ir.name.clone(),
                        ir.version.clone(),
                        arch.clone(),
                    );

                    // create the full path
                    let manifest_dir = manifest_json.split("manifest.json").nth(0).unwrap();
                    log.info(&format!("manifest directory {}", manifest_dir));
                    fs::create_dir_all(manifest_dir).expect("unable to create manifest directory");
                    log.trace(&format!("manifest json file {}", manifest_json));

                    let mnfst_url = &format!(
                        "https://{}/v2/{}/{}/manifests/{}",
                        ir.registry,
                        ir.namespace,
                        ir.name,
                        m.digest.as_ref().unwrap()
                    );

                    let manifest = reg_con
                        .get_manifest(mnfst_url.clone(), token.as_ref().unwrap().clone())
                        .await;

                    if manifest.is_ok() {
                        let working_dir_cache = format!(
                            "{}/{}/{}/{}/cache",
                            dir.clone(),
                            ir.name.clone(),
                            ir.version.clone(),
                            arch.clone(),
                        );

                        let cache_exists = Path::new(&working_dir_cache).exists();
                        let res_manifest_in_mem =
                            parse_json_manifest_operator(manifest.as_ref().unwrap().clone())
                                .unwrap();
                        let mut exists = true;
                        if cache_exists {
                            let manifest_on_disk = fs::read_to_string(&manifest_json).unwrap();
                            let res_manifest_on_disk =
                                parse_json_manifest_operator(manifest_on_disk).unwrap();
                            if res_manifest_on_disk != res_manifest_in_mem || !cache_exists {
                                exists = false;
                            }
                        } else {
                            exists = false;
                        }
                        if !exists {
                            log.info("detected change in index manifest");
                            fs::write(manifest_json.clone(), manifest.as_ref().unwrap().clone())
                                .expect("unable to write (index) manifest.json file");

                            // detected a change so clean the dir contents
                            if cache_exists {
                                rm_rf::remove(&working_dir_cache)
                                    .expect("should delete current untarred cache");
                                // re-create the cache directory
                                let mut builder = DirBuilder::new();
                                builder.mode(0o777);
                                builder
                                    .create(&working_dir_cache)
                                    .expect("unable to create directory");
                            }

                            let mut fslayers: Vec<FsLayer> = vec![];
                            for l in res_manifest_in_mem.layers.unwrap().iter() {
                                let fsl = FsLayer {
                                    blob_sum: l.digest.clone(),
                                    original_ref: Some(ir.name.clone()),
                                    size: Some(l.size),
                                };
                                fslayers.insert(0, fsl);
                            }

                            let blobs_url = format!(
                                "https://{}/v2/{}/{}/blobs/",
                                ir.registry, ir.namespace, ir.name
                            );
                            // use a concurrent process to get related blobs
                            let response = reg_con
                                .get_blobs(
                                    log,
                                    blobs_dir.clone(),
                                    blobs_url,
                                    token.as_ref().unwrap().clone(),
                                    fslayers.clone(),
                                )
                                .await;
                            log.debug(&format!("completed image index download {:#?}", response));

                            untar_layers(
                                log,
                                blobs_dir.clone(),
                                working_dir_cache.clone(),
                                fslayers.clone(),
                            )
                            .await;
                            log.hi("completed untar of layers");
                            // find the directory 'configs'
                            let config_dir =
                                find_dir(log, working_dir_cache.clone(), "configs".to_string())
                                    .await;
                            log.mid(&format!(
                                "full path for directory 'configs' {} ",
                                &config_dir
                            ));

                            DeclarativeConfig::build_updated_configs(log, config_dir.clone())
                                .expect("should build updated configs");
                        }
                    } else {
                        log.error(&format!(
                            "manifest api call {:#?}",
                            manifest.as_ref().err().unwrap()
                        ));
                    }

                    // as all architecture index files are identical
                    // it's ok to get one architecture as reference
                    if arch.clone() == "amd64" {
                        break;
                    }
                }
            }
        }

        // index is in place now get all packages, bundles and related images
        // as specified in the imagesetconfig. At this point we have the latest configs
        // folder (donwloaded and extracted in the previous step)
        // we can confidently now extract all relevant bundles

        let working_dir_cache = format!(
            "{}/{}/{}/{}/cache",
            dir.clone(),
            ir.name.clone(),
            ir.version.clone(),
            "amd64".to_string(),
        );

        let config_dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
        for operator in operators.iter() {
            // iterate through all packages in imagesetconfig
            for pkg in operator.packages.clone().unwrap() {
                let dc_map = DeclarativeConfig::get_declarativeconfig_map(
                    config_dir.clone() + &"/" + &pkg.name.clone() + &"/updated-configs/",
                );

                log.hi(&format!("operator {:#?}", pkg.name));

                let bundles = pkg.bundles;
                let mut vec_bundles: Vec<String> = vec![];

                // if no bundles found in the imagesetconfig
                // then get the head of the default channel
                if bundles.is_none() {
                    let pkg_key = pkg.name.clone() + &"=olm.package";
                    let package = dc_map.get(&pkg_key);
                    if package.is_some() {
                        // if we dont have default channel here its basically broken
                        // bye bye world :(
                        let channel_key = format!(
                            "{}=olm.channel",
                            package.unwrap().default_channel.as_ref().unwrap()
                        );
                        log.lo(&format!(
                            "  default channel {:#?}",
                            channel_key.clone().split("=").nth(0).unwrap()
                        ));
                        let channel = dc_map.get(&channel_key);
                        if channel.is_some() {
                            let mut vec_entries: Vec<String> = Vec::new();
                            let chn = channel.unwrap();
                            let entries = chn.entries.as_ref().unwrap();
                            for entry in entries.iter() {
                                vec_entries.insert(0, entry.name.clone());
                            }
                            vec_entries.sort_by(|a, b| b.cmp(a));
                            log.lo(&format!("  channel head {:#?}", vec_entries[0]));
                            vec_bundles.insert(0, vec_entries[0].to_string());
                        }
                    }
                } else {
                    // we have bundles in the imagesetconfig
                    for b in bundles.as_ref().unwrap().iter() {
                        vec_bundles.insert(0, b.name.clone());
                    }
                }

                // iterate for each bundle
                for bundle_name in vec_bundles {
                    let key = bundle_name.clone() + &"=olm.bundle".to_string();
                    let bundle = dc_map.get(&key);
                    if bundle.is_some() {
                        log.debug(&format!("bundle from dc_map {:#?}", bundle));
                        // we can  get all related images
                        let related_images = bundle.unwrap().related_images.clone().unwrap();
                        for ri in related_images.iter() {
                            image_vec.insert(0, ri.image.clone());
                            let ir_pkg = parse_url(log, ri.image.clone());
                            let mut manifest: String = String::new();
                            if !skip_manifests_check {
                                let url = &format!(
                                    "https://{}/v2/{}/{}/manifests/{}",
                                    ir_pkg.registry, ir_pkg.namespace, ir_pkg.name, ir_pkg.version
                                );

                                log.ex(&format!(
                                    "checking manifest {:#?}",
                                    ir_pkg.namespace.clone() + "/" + &ir_pkg.name
                                ));

                                let res = reg_con
                                    .get_manifest(url.clone(), token.as_ref().unwrap().to_string())
                                    .await;
                                if res.is_ok() {
                                    manifest = res.unwrap().clone();
                                    let f = &format!(
                                        "{}/manifests/operator/{}-list.json",
                                        dir.clone(),
                                        ir_pkg.version.clone()
                                    );
                                    fs::write(f, manifest.clone()).expect("unable to write file");
                                }
                            } else {
                                let manifest_file = format!(
                                    "{}/manifests/operator/{}-list.json",
                                    dir.clone(),
                                    ir_pkg.version
                                );
                                manifest = fs::read_to_string(manifest_file)
                                    .expect("should read manifest list");
                            }

                            // check to see if the manifest on disk (operator-rerence-image exists
                            // and has not changed)
                            // check for manifest list first
                            let manifest_list = parse_json_manifestlist(manifest.clone());
                            log.trace(&format!("manifest list {:#?}", manifest_list));
                            let mut fslayers: Vec<FsLayer> = Vec::new();
                            if manifest_list.is_ok() {
                                let ml = manifest_list.unwrap().clone();
                                log.trace(&format!("manifest list detected {:#?}", ml));
                                if ml.media_type
                                    == "application/vnd.docker.distribution.manifest.list.v2+json"
                                {
                                    let tmp_digest =
                                        get_sha_from_contents(manifest.clone().as_bytes());
                                    let digest = format!("sha256:{}", tmp_digest.clone());

                                    if digest != ir_pkg.version {
                                        log.warn(&format!(
                                            "digest does not match {} : {}",
                                            digest, ir_pkg.version
                                        ));
                                    }
                                    let img_ref = MirrorImageInfo {
                                        reference: ir.name.clone() + &"/" + &ir.version,
                                        name: pkg.name.clone(),
                                        arch: "all".to_string(),
                                        tag: None,
                                        namespace: ir_pkg.namespace.clone() + &"/" + &ir_pkg.name,
                                        digest: ir_pkg.version,
                                        manifest_type: "list".to_string(),
                                        created: "".to_string(),
                                        mirror_type: "operator".to_string(),
                                        bundle: Some(bundle_name.clone()),
                                    };
                                    image_ref_tracker.insert(0, img_ref.clone());
                                    // look for the digest
                                    // loop through each manifest
                                    for mf in ml.manifests.iter() {
                                        let arch_img = parse_image(log, ri.image.clone());
                                        let f = &format!(
                                            "{}/manifests/operator/{}-{}.json",
                                            dir.clone(),
                                            mf.digest.as_ref().unwrap(),
                                            mf.platform.clone().unwrap().architecture,
                                        );
                                        let mut local_manifest: String = String::new();
                                        if !skip_manifests_check {
                                            let arch_mnfst_url = format!(
                                                "https://{}/v2/{}/{}/manifests/{}",
                                                arch_img.registry,
                                                arch_img.namespace,
                                                arch_img.name,
                                                mf.digest.as_ref().unwrap()
                                            );

                                            log.mid(&format!(
                                                "api call for manifest {:#?}",
                                                f.clone()
                                            ));
                                            // use the RegistryInterface to make the api call
                                            let res = reg_con
                                                .get_manifest(
                                                    arch_mnfst_url.clone(),
                                                    token.as_ref().unwrap().to_string(),
                                                )
                                                .await;
                                            if res.is_ok() {
                                                fs::write(f, res.as_ref().unwrap())
                                                    .expect("unable to write arch manifest file");
                                                local_manifest = res.unwrap();
                                            }
                                        } else {
                                            local_manifest = fs::read_to_string(f)
                                                .expect("should read local arch manifest file");
                                        }
                                        // remove registry from related image
                                        let img_ref = MirrorImageInfo {
                                            reference: ir.name.clone() + &"/" + &ir.version,
                                            name: ri.name.clone(),
                                            arch: mf.platform.clone().unwrap().architecture,
                                            tag: None,
                                            namespace: arch_img.namespace + &"/" + &arch_img.name,
                                            digest: mf.digest.as_ref().unwrap().to_string(),
                                            manifest_type: "manifest".to_string(),
                                            created: "".to_string(),
                                            mirror_type: "operator".to_string(),
                                            bundle: Some(bundle_name.clone()),
                                        };
                                        image_ref_tracker.insert(0, img_ref.clone());

                                        // convert op_manifest.layer to FsLayer and add it to the collection
                                        let op_manifest =
                                            parse_json_manifest_operator(local_manifest.clone());

                                        if op_manifest.is_ok() {
                                            // changed to ensure no duplicates included using for..in
                                            let l = op_manifest.as_ref().unwrap().clone();
                                            for layer in l.layers.unwrap().iter() {
                                                let fslayer = FsLayer {
                                                    blob_sum: layer.digest.clone(),
                                                    original_ref: Some(ri.image.clone()),
                                                    size: Some(layer.size),
                                                };
                                                fslayers.insert(0, fslayer);
                                            }
                                            let config = op_manifest.unwrap().config.unwrap();
                                            let cfg = FsLayer {
                                                blob_sum: config.digest.clone(),
                                                original_ref: Some(ri.image.clone()),
                                                size: Some(config.size),
                                            };
                                            fslayers.insert(0, cfg);
                                        } else {
                                            log.error(&format!(
                                                "{:#?}",
                                                op_manifest.err().unwrap()
                                            ));
                                        }
                                    }
                                }
                            } else {
                                // handle manifest's here, typically in a package
                                // bundles are manifests and not multiarch (set in manifestlist)
                                let op_manifest = parse_json_manifest_operator(manifest.clone());
                                if op_manifest.is_ok() {
                                    let op_mnfst = op_manifest.unwrap();
                                    log.debug(&format!(
                                        "op_manifest {:#?} {:#?}",
                                        op_mnfst, ir_pkg.name
                                    ));
                                    let digest = get_sha_from_contents(manifest.clone().as_bytes());
                                    if digest != ir_pkg.version.split(":").nth(1).unwrap() {
                                        log.warn(&format!(
                                            "digest does not match {} : {}",
                                            ir_pkg.name, digest
                                        ));
                                    }
                                    let f = &format!(
                                        "{}/manifests/operator/{}-{}.json",
                                        dir.clone(),
                                        ir_pkg.version,
                                        "all".to_string()
                                    );
                                    fs::write(f, manifest.clone()).expect("unable to write file");
                                    let img_ref = MirrorImageInfo {
                                        reference: ir.name.clone() + &"/" + &ir.version,
                                        name: pkg.name.clone(),
                                        arch: "all".to_string(),
                                        namespace: ir_pkg.namespace.clone() + &"/" + &ir_pkg.name,
                                        digest: ir_pkg.version,
                                        manifest_type: "manifest".to_string(),
                                        tag: None,
                                        created: "".to_string(),
                                        mirror_type: "operator".to_string(),
                                        bundle: Some(bundle_name.clone()),
                                    };
                                    image_ref_tracker.insert(0, img_ref.clone());

                                    for layer in op_mnfst.layers.unwrap().iter() {
                                        let fslayer = FsLayer {
                                            blob_sum: layer.digest.clone(),
                                            original_ref: Some(ri.image.clone()),
                                            size: Some(layer.size),
                                        };
                                        fslayers.insert(0, fslayer);
                                    }
                                    // add configs
                                    let config = op_mnfst.config.unwrap();
                                    let cfg = FsLayer {
                                        blob_sum: config.digest.clone(),
                                        original_ref: Some(ri.image.clone()),
                                        size: Some(config.size),
                                    };
                                    fslayers.insert(0, cfg);
                                } else {
                                    log.error(&format!("{:#}", op_manifest.err().unwrap()));
                                    continue;
                                }
                            }

                            let op_url = format!(
                                "https://{}/v2/{}/{}/blobs/",
                                ir_pkg.registry, ir_pkg.namespace, ir_pkg.name
                            );
                            // batch the calls
                            futs.push(reg_con.get_blobs(
                                log,
                                blobs_dir.clone(),
                                op_url,
                                token.as_ref().unwrap().to_string(),
                                fslayers,
                            ));
                            if futs.len() >= batch_size {
                                let res = futs.next().await.unwrap();
                                log.debug(&format!(
                                    "completed batch of {} {:#?}",
                                    batch_size,
                                    res.unwrap()
                                ));
                            }

                            // wait for the remaining to finish.
                            while let Some(response) = futs.next().await {
                                log.debug(&format!(
                                    "completed rest of batch {:#?}",
                                    response.unwrap()
                                ));
                            }
                        }
                    } else {
                        log.error(&format!("bundle not found {:#?}", bundle));
                    }
                }
            }
        }
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs::write(
        dir.clone() + &"/mirror-metadata/operator-image-reference.json",
        serialized_manifest,
    )
    .expect("should write image reference json");

    if dry_run {
        let mut buf = String::from("");
        let data =
            fs::read_to_string(dir.clone() + &"/mirror-metadata/operator-image-reference.json");
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
                fs::write(dir.clone() + "/mappings/operator-mapping.txt", buf)
                    .expect("should write operatir-mapping.txt file");
                log.info(&format!(
                    "created operator mapping file in folder {}",
                    dir.clone() + &"/mappings/",
                ));
            } else {
                log.error(&format!(
                    "parsing operator metadata file {:#}",
                    air.err().unwrap().to_string().to_lowercase()
                ));
            }
        } else {
            log.error(&format!(
                "reading operator metadata file {:#}",
                data.err().unwrap().to_string().to_lowercase()
            ));
        }
    }
}

// get_sha_from_contents
pub fn get_sha_from_contents(manifest_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest_bytes);
    let hash_bytes = hasher.finalize();
    encode(hash_bytes)
}

// parse_index - best attempt to parse image index and return ImageReference
pub fn parse_index(log: &Logging, operators: Vec<Operator>) -> Vec<ImageReference> {
    let mut image_refs = vec![];
    for ops in operators.iter() {
        let img = ops.catalog.clone();
        log.trace(&format!("catalogs {:#?}", img));
        let mut hld = img.split("/");
        let reg = hld.nth(0).unwrap();
        let ns = hld.nth(0).unwrap();
        let index = hld.nth(0).unwrap();
        let mut i = index.split(":");
        let name = i.nth(0).unwrap();
        let ver = i.nth(0).unwrap();
        let ir = ImageReference {
            registry: reg.to_string(),
            namespace: ns.to_string(),
            name: name.to_string(),
            version: ver.to_string(),
        };
        log.debug(&format!("image reference {:#?}", img));
        image_refs.insert(0, ir);
    }
    image_refs
}

// parse_image - best attempt to parse image url and return ImageReference
pub fn parse_url(log: &Logging, img: String) -> ImageReference {
    let mut hld = img.split("/");
    let reg = hld.nth(0).unwrap();
    let ns = hld.nth(0).unwrap();
    let index = hld.nth(0).unwrap();
    let mut i = index.split(":");
    let name = i.nth(0).unwrap();
    let ver = i.nth(0).unwrap();
    let ir = ImageReference {
        registry: reg.to_string(),
        namespace: ns.to_string(),
        name: name.split("@").nth(0).unwrap().to_string(),
        version: "sha256:".to_owned() + ver,
    };
    log.trace(&format!("image reference {:#?}", img));
    ir
}

// parse the manifest json for operator indexes only
pub fn parse_json_manifest_operator(data: String) -> Result<Manifest, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: Manifest = serde_json::from_str(&data)?;
    Ok(root)
}

// parse the manifest json for operator indexes only
pub fn parse_json_manifestlist(data: String) -> Result<ManifestList, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: ManifestList = serde_json::from_str(&data)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use async_trait::async_trait;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn get_related_images_from_catalog_with_channel_pass() {
        let log = &Logging {
            log_level: Level::TRACE,
        };
        let bundle = Bundle {
            name: String::from("aws-load-balancer-operator-bundle"),
        };
        let vec_bundle = vec![bundle];
        let _pkg = Package {
            name: String::from("some-operator"),
            bundles: Some(vec_bundle),
        };

        let ir1 = RelatedImage {
            name: String::from("controller"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:d7bc364512178c36671d8a4b5a76cf7cb10f8e56997106187b0fe1f032670ece"),
        };
        let ir2 = RelatedImage {
            name: String::from(""),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:50b9402635dd4b312a86bed05dcdbda8c00120d3789ec2e9b527045100b3bdb4"),
        };
        let ir3 = RelatedImage {
            name: String::from("aws-load-balancer-rhel8-operator-95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d-annotation"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"),
        };
        let ir4 = RelatedImage {
            name: String::from("manager"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"),
        };
        let ir5 = RelatedImage {
            name: String::from("kube-rbac-proxy"),
            image: String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:3658954f199040b0f244945c94955f794ee68008657421002e1b32962e7c30fc"),
        };
        let ri_vec = vec![ir1, ir2, ir3, ir4, ir5];
        log.trace(&format!("results {:#?}", ri_vec));
        /*
        let matching = res
            .iter()
            .zip(&wrapper_vec)
            .filter(|&(res, wrapper)| res.images.len() == wrapper.images.len())
            .count();
        assert_eq!(matching, 1);
        for x in res.iter() {
            assert_eq!(x.images[0].image, String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:d7bc364512178c36671d8a4b5a76cf7cb10f8e56997106187b0fe1f032670ece"));
            assert_eq!(x.images[1].image, String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:50b9402635dd4b312a86bed05dcdbda8c00120d3789ec2e9b527045100b3bdb4"));
            assert_eq!(x.images[2].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"));
            assert_eq!(x.images[3].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"));
            assert_eq!(x.images[4].image, String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:3658954f199040b0f244945c94955f794ee68008657421002e1b32962e7c30fc"));
        }
        */
    }

    #[test]
    fn get_related_images_from_catalog_no_channel_pass() {
        let _log = &Logging {
            log_level: Level::INFO,
        };
        let bundle = Bundle {
            name: String::from("aws-load-balancer-operator-bundle"),
        };
        let vec_bundle = vec![bundle];
        let _pkg = Package {
            name: String::from("some-operator"),
            bundles: Some(vec_bundle),
        };

        let ir1 = RelatedImage {
            name: String::from("controller"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92"),
        };
        let ir2 = RelatedImage {
            name: String::from(""),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d"),
        };
        let ir3 = RelatedImage {
            name: String::from("aws-load-balancer-rhel8-operator-95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d-annotation"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"),
        };
        let ir4 = RelatedImage {
            name: String::from("manager"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"),
        };
        let ir5 = RelatedImage {
            name: String::from("kube-rbac-proxy"),
            image: String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6"),
        };
        let _ri_vec = vec![ir1, ir2, ir3, ir4, ir5];
    }

    #[test]
    fn mirror_to_disk_pass() {
        let log = &Logging {
            log_level: Level::DEBUG,
        };

        // we set up a mock server for the auth-credentials
        let mut server = mockito::Server::new();
        let url = server.url();

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
        let bundle = Bundle {
            name: String::from("aws-load-balancer-operator-bundle"),
        };
        let vec_bundle = vec![bundle];

        let pkg = Package {
            name: String::from("some-operator"),
            bundles: Some(vec_bundle),
        };

        let pkgs = vec![pkg];
        let op = Operator {
            catalog: String::from(url.replace("http://", "") + "/test/test-index-operator:v1.0"),
            packages: Some(pkgs),
        };

        #[derive(Clone)]
        struct Fake {}

        #[async_trait]
        impl RegistryInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, Box<dyn std::error::Error>> {
                let mut content = String::from("");

                if url.contains("test-index-operator") {
                    content =
                        fs::read_to_string("test-artifacts/test-index-operator/v1.0/manifest.json")
                            .expect("should read operator-index manifest file")
                }
                if url.contains("cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92")
                {
                    content =
                        fs::read_to_string("test-artifacts/simulate-api-call/manifest-list.json")
                            .expect("should read test (albo) controller manifest-list file");
                }
                if url.contains("75012e910726992f70c892b11e50e409852501c64903fa05fa68d89172546d5d")
                    | url.contains(
                        "5e03f571c5993f0853a910b7c0cab44ec0e451b94a9677ed82e921b54a4b735a",
                    )
                {
                    content =
                        fs::read_to_string("test-artifacts/simulate-api-call/manifest-amd64.json")
                            .expect("should read test (albo) controller manifest-am64 file");
                }
                if url.contains("d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d")
                {
                    content = fs::read_to_string("test-artifacts/simulate-api-call/manifest.json")
                        .expect("should read test (albo) bundle manifest file");
                }
                if url.contains("cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0")
                {
                    content = fs::read_to_string(
                        "test-artifacts/simulate-api-call/manifest-amd64-operator.json",
                    )
                    .expect("should read test (albo) operator manifest file");
                }
                if url.contains("422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6")
                {
                    content = fs::read_to_string(
                        "test-artifacts/simulate-api-call/manifest-amd64-kube.json",
                    )
                    .expect("should read test (openshift) kube-proxy manifest file");
                }

                Ok(content)
            }

            async fn get_blobs(
                &self,
                log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _layers: Vec<FsLayer>,
            ) -> Result<String, Box<dyn std::error::Error>> {
                log.info("testing logging in fake test");
                Ok(String::from("test"))
            }

            async fn push_image(
                &self,
                log: &Logging,
                _dir: String,
                _subdir: String,
                _url: String,
                _token: String,
                _manifest: Manifest,
            ) -> Result<String, MirrorError> {
                log.info("testing logging in fake test");
                Ok(String::from("test"))
            }
        }

        let fake = Fake {};

        let ops = vec![op.clone()];
        aw!(operator_mirror_to_disk(
            fake.clone(),
            log,
            String::from("./test-artifacts/"),
            true,
            true,
            ops.clone()
        ));
    }
}
