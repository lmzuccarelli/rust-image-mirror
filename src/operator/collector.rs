use crate::api::schema::MirrorImageInfo;
use crate::batch::worker::execute_batch;
use crate::catalog::builder::*;
use crate::config::load::*;
use crate::mirror::utils::{
    fs_handler, parse_image, parse_json_manifest_operator, parse_json_metadata,
    process_and_update_manifest, remove_duplicates,
};
use crate::MirrorParameters;
use custom_logger::*;
use hex::encode;
use mirror_auth::*;
use mirror_catalog::*;
use mirror_catalog_index::*;
use mirror_copy::*;
use mirror_error::MirrorError;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

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
    operators: Vec<Operator>,
    mp: MirrorParameters,
) -> Result<(), MirrorError> {
    log.hi("operator collector mode: mirror-to-disk");

    // set up dir to store all manifests
    fs_handler(
        format!("{}/{}", mp.dir.clone(), "manifests/operator".to_string()),
        "create_dir",
        None,
    )
    .await?;

    // parse the config - iterate through each catalog
    let img_ref = parse_index(log, operators.clone());
    log.debug(&format!("image refs {:#?}", img_ref));
    let blobs_dir = mp.dir.clone() + "/blobs-store/";
    let mut image_vec: Vec<String> = Vec::new();
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();
    let mut vec_catalog_info: Vec<CatalogCopyInfo> = Vec::new();
    let mut manifestlist: String;

    // get all relevant catalogs in config
    // download manifests and blobs if changed
    // untar and set /configs directory
    for ir in img_ref.iter() {
        let manifestlist_json = format!(
            "{}/{}/{}/manifest-list.json",
            mp.dir.clone(),
            ir.name.clone(),
            ir.version.clone(),
        );
        // use token to get manifest
        let token = get_token(log, ir.registry.clone()).await?;
        log.trace(&format!(
            "[operator_mirror_to_disk] manifest json file {}",
            manifestlist_json
        ));
        if !(mp.skip_manifest_check == "operator" || mp.skip_manifest_check == "all") {
            // construct manifest api url
            let manifest_url = &format!(
                "https://{}/v2/{}/{}/manifests/{}",
                ir.registry, ir.namespace, ir.name, ir.version
            );

            log.mid(&format!(
                "[operator_mirror_to_disk] api call manifest for {}",
                format!(
                    "{}/{}/{}/{}",
                    ir.registry, ir.namespace, ir.name, ir.version
                )
            ));

            let mfstlist_dir = format!(
                "{}/{}/{}",
                mp.dir.clone(),
                ir.name.clone(),
                ir.version.clone()
            );

            // this should get a manifestlist
            let res = reg_con
                .get_manifest(manifest_url.clone(), token.clone())
                .await?;
            fs_handler(mfstlist_dir, "create_dir", None).await?;
            let res_manifestlist = process_and_update_manifest(
                log,
                res.clone(),
                manifestlist_json.clone(),
                mp.clone().generic_override,
            )
            .await?;
            log.warn(&format!(
                "[operator_mirror_to_disk] result from api call {}",
                res.clone()
            ));
            if res_manifestlist.is_some() {
                manifestlist = fs_handler(res_manifestlist.unwrap().clone(), "read", None).await?;
            } else {
                manifestlist = res.clone();
            }
        } else {
            // try read from disk
            manifestlist = fs_handler(manifestlist_json.clone(), "read", None).await?;
        }
        let local_manifestlist = manifestlist.clone();
        let local_pml = parse_json_manifestlist(local_manifestlist.clone())?;
        for m in local_pml.clone().manifests.iter() {
            let arch = m.platform.as_ref().unwrap().architecture.to_string();
            let manifest_json = format!(
                "{}/{}/{}/{}/manifest.json",
                mp.dir.clone(),
                ir.name.clone(),
                ir.version.clone(),
                arch.clone(),
            );

            // create the full path
            let manifest_dir = manifest_json.split("manifest.json").nth(0).unwrap();
            log.info(&format!(
                "[operator_mirror_to_disk] manifest directory {}",
                manifest_dir
            ));
            fs_handler(manifest_dir.to_string(), "create_dir", None).await?;
            let mnfst_url = &format!(
                "https://{}/v2/{}/{}/manifests/{}",
                ir.registry,
                ir.namespace,
                ir.name,
                m.digest.as_ref().unwrap()
            );
            let manifest = reg_con
                .get_manifest(mnfst_url.clone(), token.clone())
                .await?;
            let working_dir_cache = format!(
                "{}/{}/{}/{}/cache",
                mp.dir.clone(),
                ir.name.clone(),
                ir.version.clone(),
                arch.clone(),
            );
            let cache_exists = Path::new(&working_dir_cache).exists();
            log.debug(&format!(
                "[operator_mirror_to_disk] main operator manifest file {}",
                manifest_json
            ));
            if cache_exists {
                let changed = process_and_update_manifest(
                    log,
                    manifest.clone(),
                    manifest_json.clone(),
                    mp.clone().generic_override,
                )
                .await?;
                if changed.is_some() {
                    log.info("[operator_mirror_to_disk] detected change in manifest");
                    let changed_manifest =
                        fs_handler(changed.unwrap().clone(), "read", None).await?;
                    let res_pm = parse_json_manifest_operator(changed_manifest.clone())?;
                    // detected a change so clean the dir contents
                    rm_rf::remove(&working_dir_cache)
                        .expect("[operator_mirror_to_disk] should delete current untarred cache");
                    // re-create the cache directory
                    let mut builder = DirBuilder::new();
                    builder.mode(0o777);
                    builder
                        .create(&working_dir_cache)
                        .expect("[operator_mirror_to_disk] unable to create directory");
                    let mut fslayers: Vec<FsLayer> = vec![];
                    for l in res_pm.clone().layers.unwrap().iter() {
                        let fsl = FsLayer {
                            blob_sum: l.digest.clone(),
                            original_ref: Some(ir.name.clone()),
                            size: Some(l.size),
                            number: None,
                        };
                        fslayers.insert(0, fsl);
                    }
                    let blobs_url = format!(
                        "https://{}/v2/{}/{}/blobs/",
                        ir.registry, ir.namespace, ir.name
                    );
                    let mut hm: HashMap<String, Vec<FsLayer>> = HashMap::new();
                    hm.insert(blobs_url, fslayers.clone());
                    // use a concurrent process to get related blobs
                    execute_batch(log, blobs_dir.clone(), mp.verify_blobs, mp.tls_verify, hm)
                        .await?;
                    log.debug(&format!(
                        "[operator_mirror_to_disk] completed image index download"
                    ));
                    log.debug(&format!(
                        "[operator_mirror_to_disk] map {:#?}",
                        fslayers.clone(),
                    ));

                    untar_layers(
                        log,
                        blobs_dir.clone(),
                        working_dir_cache.clone(),
                        fslayers.clone(),
                    )
                    .await;
                    log.hi("[operator_mirror_to_disk] completed untar of layers");
                    // find the directory 'configs'
                    let config_dir =
                        find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
                    log.mid(&format!(
                        "[operator_mirror_to_disk] full path for directory 'configs' {} ",
                        &config_dir
                    ));
                    DeclarativeConfig::build_updated_configs(log, config_dir.clone())
                        .expect("[operator_mirror_to_disk] should build updated configs");
                }
            }

            // as all architecture index files are identical
            // it's ok to get one architecture as reference
            if arch.clone() == "amd64" {
                break;
            }
        }

        // index is in place now get all packages, bundles and related images
        // as specified in the imagesetconfig. At this point we have the latest configs
        // folder (donwloaded and extracted in the previous step)
        // we can confidently now extract all relevant bundles

        let working_dir_cache = format!(
            "{}/{}/{}/{}/cache",
            mp.dir.clone(),
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
                log.mid(&format!(
                    "[operator_mirror_to_disk] operator {:#?}",
                    pkg.name
                ));
                log.debug(&format!(
                    "[operator_mirror_to_disk] bundles {:#?}",
                    pkg.bundles
                ));
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
                let mut found_channel: String = String::new();
                for bundle_name in vec_bundles {
                    let key = bundle_name.clone() + &"=olm.bundle".to_string();
                    let bundle = dc_map.get(&key);
                    log.trace(&format!("dc keys {:#?}", dc_map.keys()));
                    if bundle.is_some() {
                        log.info(&format!(
                            "[operator_mirror_to_disk] bundle name {:#?}",
                            bundle_name.clone()
                        ));
                        // get the relevant channels
                        for (k, v) in dc_map.iter() {
                            if k.contains("olm.channel") {
                                for e in v.entries.as_ref().unwrap().iter() {
                                    if e.name.contains(&bundle_name.clone()) {
                                        found_channel = k.split("=").nth(0).unwrap().to_string();
                                        break;
                                    }
                                }
                            }
                        }
                        // we can  get all related images
                        let related_images = bundle.unwrap().related_images.clone().unwrap();
                        for ri in related_images.iter() {
                            image_vec.insert(0, ri.image.clone());
                            let ir_pkg = parse_url(log, ri.image.clone());
                            let manifest: String;
                            if !(mp.skip_manifest_check == "operator"
                                || mp.skip_manifest_check == "all")
                            {
                                let url = &format!(
                                    "https://{}/v2/{}/{}/manifests/{}",
                                    ir_pkg.registry, ir_pkg.namespace, ir_pkg.name, ir_pkg.version
                                );
                                log.ex(&format!(
                                    "[operator_mirror_to_disk] checking manifest {:#?}",
                                    ir_pkg.namespace.clone() + "/" + &ir_pkg.name
                                ));
                                log.debug(&format!(
                                    "[operator_mirror_to_disk] related image in bundle {}",
                                    ri.image.clone()
                                ));
                                let res = reg_con.get_manifest(url.clone(), token.clone()).await?;
                                let f = &format!(
                                    "{}/manifests/operator/{}-list.json",
                                    mp.dir.clone(),
                                    ir_pkg.version.clone()
                                );
                                fs_handler(f.to_string(), "write", Some(res.clone())).await?;
                                manifest = res.clone();
                            } else {
                                let manifest_file = format!(
                                    "{}/manifests/operator/{}-list.json",
                                    mp.dir.clone(),
                                    ir_pkg.version
                                );
                                manifest = fs_handler(manifest_file, "read", None).await?;
                            }

                            // check to see if the manifest on disk (operator-reference-image exists
                            // and has not changed)
                            // check for manifest list first
                            let manifest_list = parse_json_manifestlist(manifest.clone());
                            let mut fslayers: Vec<FsLayer> = Vec::new();
                            if manifest_list.is_ok() {
                                let ml = manifest_list.unwrap().clone();
                                log.trace(&format!(
                                    "[operator_mirror_to_disk] manifest list detected {:#?}",
                                    ml
                                ));
                                if ml.media_type
                                    == "application/vnd.docker.distribution.manifest.list.v2+json"
                                {
                                    let tmp_digest =
                                        get_sha_from_contents(manifest.clone().as_bytes());
                                    let digest = format!("sha256:{}", tmp_digest.clone());

                                    if digest != ir_pkg.version {
                                        log.warn(&format!(
                                            "[operator_mirror_to_disk] digest does not match {} : {}",
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
                                            mp.dir.clone(),
                                            mf.digest.as_ref().unwrap(),
                                            mf.platform.clone().unwrap().architecture,
                                        );
                                        let local_manifest: String;
                                        if !(mp.skip_manifest_check == "operator"
                                            || mp.skip_manifest_check == "all")
                                        {
                                            let arch_mnfst_url = format!(
                                                "https://{}/v2/{}/{}/manifests/{}",
                                                arch_img.registry,
                                                arch_img.namespace,
                                                arch_img.name,
                                                mf.digest.as_ref().unwrap()
                                            );

                                            log.mid(&format!(
                                                "[operator_mirror_to_disk] api call for manifest {}/{}",
                                                arch_img.namespace.clone(),
                                                arch_img.name.clone()
                                            ));
                                            // use the RegistryInterface to make the api call
                                            let res = reg_con
                                                .get_manifest(arch_mnfst_url.clone(), token.clone())
                                                .await?;
                                            fs_handler(f.to_string(), "write", Some(res.clone()))
                                                .await?;
                                            local_manifest = res.clone();
                                        } else {
                                            local_manifest =
                                                fs_handler(f.to_string(), "read", None).await?
                                        }
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
                                            parse_json_manifest_operator(local_manifest.clone())?;
                                        // changed to ensure no duplicates included using for..in
                                        let l = op_manifest.clone();
                                        for layer in l.layers.unwrap().iter() {
                                            let fslayer = FsLayer {
                                                blob_sum: layer.digest.clone(),
                                                original_ref: Some(ri.image.clone()),
                                                size: Some(layer.size),
                                                number: None,
                                            };
                                            fslayers.insert(0, fslayer);
                                        }
                                        let config = op_manifest.clone().config.unwrap();
                                        let cfg = FsLayer {
                                            blob_sum: config.digest.clone(),
                                            original_ref: Some(ri.image.clone()),
                                            size: Some(config.size),
                                            number: None,
                                        };
                                        fslayers.insert(0, cfg);
                                    }
                                }
                            } else {
                                // handle manifest's here, typically in a package
                                // bundles are manifests and not multiarch (set in manifestlist)
                                let op_manifest = parse_json_manifest_operator(manifest.clone())?;
                                let op_mnfst = op_manifest.clone();
                                log.debug(&format!(
                                    "[operator_mirror_to_disk] op_manifest {:#?}",
                                    ir_pkg.name
                                ));
                                let digest = get_sha_from_contents(manifest.clone().as_bytes());
                                if digest != ir_pkg.version.split(":").nth(1).unwrap() {
                                    log.warn(&format!(
                                        "[operator_mirror_to_disk] digest does not match {} : {}",
                                        ir_pkg.name, digest
                                    ));
                                }
                                let f = &format!(
                                    "{}/manifests/operator/{}-{}.json",
                                    mp.dir.clone(),
                                    ir_pkg.version,
                                    "all".to_string()
                                );
                                fs_handler(
                                    f.to_string(),
                                    "write",
                                    Some(manifest.clone().to_string()),
                                )
                                .await?;
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
                                        number: None,
                                    };
                                    fslayers.insert(0, fslayer);
                                }
                                // add configs
                                let config = op_mnfst.config.unwrap();
                                let cfg = FsLayer {
                                    blob_sum: config.digest.clone(),
                                    original_ref: Some(ri.image.clone()),
                                    size: Some(config.size),
                                    number: None,
                                };
                                fslayers.insert(0, cfg);
                            }

                            let op_url = format!(
                                "https://{}/v2/{}/{}/blobs/",
                                ir_pkg.registry, ir_pkg.namespace, ir_pkg.name
                            );

                            let mut in_map: HashMap<String, Vec<FsLayer>> = HashMap::new();
                            let url = mp.generic_override.get("url-override");
                            if url.is_some() {
                                let updated_url = format!(
                                    "{}/v2/{}/{}/blobs/",
                                    url.unwrap(),
                                    ir_pkg.namespace,
                                    ir_pkg.name
                                );
                                in_map.insert(updated_url.clone(), fslayers.clone());
                            } else {
                                in_map.insert(op_url.clone(), fslayers.clone());
                            }
                            let map = remove_duplicates(mp.dir.clone(), in_map);
                            execute_batch(log, mp.dir.clone(), mp.verify_blobs, mp.tls_verify, map)
                                .await?;
                        }
                    } else {
                        log.error(&format!(
                            "[operator_mirror_to_disk] bundle not found {:#?}",
                            bundle
                        ));
                    }
                }
                let cci = CatalogCopyInfo {
                    catalog: operator.catalog.clone(),
                    package: pkg.name.clone(),
                    channel: found_channel.clone(),
                };
                vec_catalog_info.insert(0, cci.clone());
            }
        }
    }

    if mp.rebuild_catalogs.is_some() && mp.rebuild_catalogs.unwrap() {
        let g_bc = ImplCatalogBuildInterface {};
        log.mid("[operator_mirror_to_disk] rebuild catalog index");
        let res = g_bc
            .build_catalog(log, mp.dir.clone(), vec_catalog_info)
            .await?;
        image_ref_tracker.append(&mut res.clone());
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs_handler(
        mp.dir.clone() + &"/mirror-metadata/operator-image-reference.json",
        "write",
        Some(serialized_manifest),
    )
    .await?;

    if mp.dry_run {
        log.info("[operator_mirror_to_disk] dry-run flag detected");
        let mut buf = String::from("");
        let data = fs_handler(
            mp.dir.clone() + &"/mirror-metadata/operator-image-reference.json",
            "read",
            None,
        )
        .await?;
        let air = parse_json_metadata(data.clone())?;
        for mii in air.clone().iter() {
            if mp.architectures.contains(&mii.arch.to_string()) {
                let src = &format!("{}{}", "docker://", mii.reference);
                let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                buf = buf + &format!("{}={}\n", src, dest);
            }
        }
        fs_handler(
            mp.dir.clone() + "/mappings/operator-mapping.txt",
            "write",
            Some(buf),
        )
        .await?;
        log.mid(&format!(
            "[operator_mirror_to_disk] created operator mapping file in folder {}",
            mp.dir.clone() + &"/mappings/",
        ));
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use async_trait::async_trait;
    use std::fs;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn operator_mirror_to_disk_pass() {
        let log = &Logging {
            log_level: Level::DEBUG,
        };

        #[derive(Clone)]
        struct Fake {}

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

        server
            .mock("GET", "/v2")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"blob\": \"test\",
                }",
            )
            .create();

        #[async_trait]
        impl RegistryInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                println!("DEBUG LMZ {}", url);
                let content = match url.clone() {
                    x if x.contains("test-index-operator/manifests/v1.0") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-index-operator/v1.0/manifest.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("test-index-operator/v2.0") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-index-operator/manifests/v2.0/manifest.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains(
                        "sha256:cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92",
                    ) =>
                    {
                        let content = fs::read_to_string(
                            "test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1/manifest-list.json",
                        )
                        .expect("should read test (albo) controller manifest-list file");
                        content
                    }
                    x if x.contains(
                        "sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0",
                    ) || x.contains(
                        "sha256:d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d",
                    ) =>
                    {
                        let content = fs::read_to_string("test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-operator-bundle/stable-v1/manifest.json")
                        .expect("should read bundle file");
                        content
                    }
                    x if x.contains(
                        "sha256:5e03f571c5993f0853a910b7c0cab44ec0e451b94a9677ed82e921b54a4b735a",
                    ) =>
                    {
                        let content = fs::read_to_string("test-artifacts/test-index-operator/v1.0/operators/albo/aws-load-balancer-controller-rhel8/stable-v1/manifest-amd64.json")
                        .expect("should read bundle file");
                        content
                    }
                    x if x.contains(
                        "sha256:422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6",
                    ) =>
                    {
                        let content = fs::read_to_string("test-artifacts/test-index-operator/v1.0/operators/openshift4/ose-kube-rbac-proxy/stable-v1/manifest.json")
                        .expect("should kube proxy file");
                        content
                    }

                    x if x.contains(
                        "sha256:65e311ef7036acc3692d291403656b840fd216d120b3c37af768f91df050257d",
                    ) =>
                    {
                        let content = fs::read_to_string(
                            "test-artifacts/test-index-operator/v1.0/amd64/manifest.json",
                        )
                        .expect("should read test (openshift) kube-proxy manifest file");
                        content
                    }
                    x if x.contains("error-operator-image/manifests/latest") => {
                        let err = MirrorError::new("forced error");
                        return Err(err);
                    }
                    _ => "".to_string(),
                };
                Ok(content.to_string())
            }

            async fn get_blobs(
                &self,
                _log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _layers: Vec<FsLayer>,
            ) -> Result<String, MirrorError> {
                Ok("ok".to_string())
            }

            async fn push_image(
                &self,
                _log: &Logging,
                _dir: String,
                _sub_component: String,
                _url: String,
                _token: String,
                _manifest: Manifest,
            ) -> Result<String, MirrorError> {
                Ok("ok".to_string())
            }
        }

        let fake = Fake {};

        fs::create_dir_all("./test-artifacts/mirror-metadata")
            .expect("should create mirror-metadata test folder");
        fs::create_dir_all("./test-artifacts/mappings")
            .expect("should create mappings test folder");
        fs::create_dir_all("./test-artifacts/blobs-store/ac")
            .expect("should create blobs-store test folder");
        fs::create_dir_all("./test-artifacts/blobs-store/5f")
            .expect("should create blobs-store test folder");
        fs::create_dir_all("./test-artifacts/manifests")
            .expect("should create manifests test folder");
        fs::create_dir_all("./test-artifacts/artifacts")
            .expect("should create artifacts test folder");
        fs::copy("test-artifacts/test-index-operator/copy-cache/5f9d3dcf5281c5f6512471366be68bee46c2485eddf4fd1887da6b240712be5f",
        "test-artifacts/blobs-store/5f/5f9d3dcf5281c5f6512471366be68bee46c2485eddf4fd1887da6b240712be5f").expect("should copy blob");

        let bundle = Bundle {
            name: String::from("aws-load-balancer-operator.v1.0.0"),
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
        let vec_op = vec![op];

        let mut mp = MirrorParameters {
            architectures: vec![
                "amd64".to_string(),
                "arm64".to_string(),
                "ppc64le".to_string(),
                "s390x".to_string(),
            ],
            destination: "".to_string(),
            dry_run: false,
            dir: "./test-artifacts".to_string(),
            from: "".to_string(),
            skip_blob_upload: false,
            skip_manifest_check: "none".to_string(),
            tls_verify: false,
            verify_blobs: false,
            generic_override: HashMap::new(),
            rebuild_catalogs: Some(false),
        };
        mp.generic_override
            .insert("url-override".to_string(), url.clone());

        log.ex(&format!("executing sub test : with bundle [should pass]"));
        let res = aw!(operator_mirror_to_disk(
            fake.clone(),
            log,
            vec_op.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);

        let pkg_nb = Package {
            name: String::from("some-operator"),
            bundles: None,
        };
        let pkgs_nb = vec![pkg_nb];
        let op_nb = Operator {
            catalog: String::from(url.replace("http://", "") + "/test/test-index-operator:v1.0"),
            packages: Some(pkgs_nb.clone()),
        };
        let vec_op_nb = vec![op_nb];

        mp.dry_run = true;
        mp.skip_manifest_check = "all".to_string();
        log.ex(&format!(
            "executing sub test : dry-run no bundle [should pass]"
        ));
        let res = aw!(operator_mirror_to_disk(
            fake.clone(),
            log,
            vec_op_nb.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);

        let op_v2 = Operator {
            catalog: String::from(url.replace("http://", "") + "/test/test-index-operator:v2.0"),
            packages: Some(pkgs_nb.clone()),
        };
        let vec_op_v2 = vec![op_v2];

        mp.dry_run = false;
        mp.skip_manifest_check = "all".to_string();
        mp.generic_override.insert(
            "./test-artifacts/test-index-operator/v2.0/amd64/manifest.json".to_string(),
            "./test-artifacts/test-index-operator/v2.0/amd64/override.json".to_string(),
        );
        log.ex(&format!("executing sub test : file override [should pass]"));
        let res = aw!(operator_mirror_to_disk(
            fake.clone(),
            log,
            vec_op_v2.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);

        fs::remove_dir_all("./test-artifacts/mirror-metadata")
            .expect("should delete mirror-metadata test folder");
        fs::remove_dir_all("./test-artifacts/mappings")
            .expect("should delete mappings test folder");
        fs::remove_dir_all("./test-artifacts/blobs-store/ac")
            .expect("should delete blobs-store test folder");
        fs::remove_dir_all("./test-artifacts/manifests")
            .expect("should delete manifests test folder");
        fs::remove_dir_all("./test-artifacts/artifacts")
            .expect("should delete artifacts test folder");
    }
}
