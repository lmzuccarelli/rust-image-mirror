use crate::batch::worker::*;
use crate::config::load::*;
use crate::graphdata::process::{GraphDataInterface, ImplGraphDataInterface};
use crate::MirrorParameters;
use chrono::{DateTime, Local};
use custom_logger::*;
use hex::encode;
use mirror_auth::{get_token, ImplTokenInterface};
use mirror_catalog_index::untar_layers;
use mirror_copy::DownloadImageInterface;
use mirror_error::MirrorError;
use mirror_utils::{
    fs_handler, parse_image, parse_json_manifestlist, process_and_update_manifest,
    process_fb_image, read_and_parse_manifest, read_and_parse_metadata,
    read_and_parse_oci_manifest, remove_duplicates, FsLayer, ImageReference, MirrorImageInfo,
};
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::usize;
use walkdir::WalkDir;
// ReleaseSchema
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
// Spec
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Spec {
    #[serde(rename = "lookupPolicy")]
    pub lookup: LookupPolicy,
    #[serde(rename = "tags")]
    pub tags: Vec<Tags>,
}
// LookupPolicy
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LookupPolicy {
    #[serde(rename = "local")]
    pub local: bool,
}
// Tags
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Tags {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "from")]
    pub from: From,
}
// From
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct From {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "kind")]
    pub kind: String,
}
// MetaData
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MetaData {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "creationTimestamp")]
    pub creation: String,
}
// ReleaseImageInfo
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReleaseImageInfo {
    pub file: String,
    pub original_ref: String,
}
// collect all operator images
pub async fn release_mirror_to_disk<T: DownloadImageInterface + Clone>(
    reg_con: T,
    log: &Logging,
    releases: Release,
    mp: MirrorParameters,
) -> Result<(), MirrorError> {
    log.hi("[release_mirror_to_disk] collector mode: mirror-to-disk");

    // set up dir to store all manifests
    fs_handler(
        format!(
            "{}/{}",
            mp.dir.clone(),
            "/manifests/ocp-release".to_string()
        ),
        "create_dir",
        None,
    )
    .await?;

    let mut vec_process_manifests: Vec<ReleaseImageInfo> = Vec::new();
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();
    let mut fslayers: HashMap<String, Vec<FsLayer>> = HashMap::new();
    let t_impl = ImplTokenInterface {};
    // parse the config
    for release in releases.images.iter() {
        // parse image index
        let index_image_ref = convert_release_image_index(log, release.name.clone());
        log.debug(&format!(
            "[release_mirror_to_disk] image refs {:#?}",
            index_image_ref.clone()
        ));
        let token = get_token(
            t_impl.clone(),
            log,
            index_image_ref.clone().registry,
            "".to_string(),
            mp.tls_verify,
        )
        .await?;
        let manifest: String;
        let release_manifest_file = format!(
            "{}/{}/{}/manifest.json",
            mp.dir.clone(),
            "ocp-release",
            index_image_ref.version
        );
        if !(mp.skip_manifest_check == "release" || mp.skip_manifest_check == "all") {
            // construct manifest api call
            let manifest_url = &format!(
                "https://{}/v2/{}/{}/manifests/{}",
                index_image_ref.registry,
                index_image_ref.namespace,
                index_image_ref.name,
                index_image_ref.version
            );
            log.info(&format!(
                "[release_mirror_to_disk] api call for manifest {}",
                release.name.clone()
            ));
            let res = reg_con
                .get_manifest(manifest_url.clone(), token.clone())
                .await?;
            let release_manifest_dir = &format!(
                "{}/{}/{}",
                mp.dir.clone(),
                "ocp-release",
                index_image_ref.version
            );
            fs_handler(release_manifest_dir.to_string(), "create_dir", None).await?;
            let changed = process_and_update_manifest(
                log,
                res.clone(),
                release_manifest_file.clone(),
                mp.generic_override.clone(),
            )
            .await?;
            if changed.is_some() {
                let rii = ReleaseImageInfo {
                    file: changed.unwrap().clone(),
                    original_ref: release.name.clone(),
                };
                vec_process_manifests.insert(0, rii);
            }
            manifest = res.clone();
        } else {
            // read from disk
            manifest = fs_handler(release_manifest_file.clone(), "read", None).await?;
        }

        // multi arch
        // in the cli ensure that only multi is entered
        // and no other platform architecture
        if release.name.clone().contains("multi") {
            let manifest_list = parse_json_manifestlist(manifest.clone())?;
            for mfst in manifest_list.manifests.iter() {
                // contruct api call for manifests
                let manifest_url = &format!(
                    "https://{}/v2/{}/{}/manifests/{}",
                    index_image_ref.clone().registry,
                    index_image_ref.clone().namespace,
                    index_image_ref.clone().name,
                    mfst.digest.as_ref().unwrap()
                );
                log.debug(&format!(
                    "[release_mirror_to_disk] checking multi arch manifest for {}",
                    release.name.clone() + "/" + mfst.digest.as_ref().unwrap()
                ));

                let original_ref = format!(
                    "{}-{}",
                    release.name.clone().split("-multi").nth(0).unwrap(),
                    mfst.platform.as_ref().unwrap().architecture
                );

                let inner_manifest = reg_con
                    .get_manifest(manifest_url.clone(), token.clone())
                    .await?;
                // create the directory to store manifests in
                let inner_manifest_json_dir = &format!(
                    "{}/{}/{}-{}",
                    mp.dir.clone(),
                    "ocp-release",
                    index_image_ref.clone().version.split("-").nth(0).unwrap(),
                    mfst.platform.as_ref().unwrap().architecture,
                );
                log.info(&format!(
                    "[release_mirror_to_disk] manifest_json_dir {}",
                    inner_manifest_json_dir.clone()
                ));
                fs_handler(inner_manifest_json_dir.to_string(), "create_dir", None).await?;
                let inner_manifest_json_file = format!("{}/manifest.json", inner_manifest_json_dir);
                // check if it exists first
                let changed = process_and_update_manifest(
                    log,
                    inner_manifest.clone(),
                    inner_manifest_json_file.clone(),
                    mp.generic_override.clone(),
                )
                .await?;
                if changed.is_some() {
                    let rii = ReleaseImageInfo {
                        file: changed.unwrap().clone(),
                        original_ref: original_ref.clone(),
                    };
                    vec_process_manifests.insert(0, rii);
                }
            }
        }

        if vec_process_manifests.clone().len() == 0 {
            log.info("[release_mirror_to_disk] no change detected in manifest files")
        }

        for mf in vec_process_manifests.clone().iter() {
            log.info("[release_mirror_to_disk] changed detected in manifest file/s");
            log.info(&format!("[release_mirror_to_disk] processing {} ", mf.file));
            log.debug(&format!(
                "[release_mirror_to_disk] original ref {} ",
                mf.original_ref
            ));
            let mut vec_fslayer: Vec<FsLayer> = Vec::new();
            //TODO: change the function convert_release...
            let image_ref = convert_release_image_index(log, mf.clone().original_ref);
            let blobs_url = &format!(
                "https://{}/v2/{}/{}/blobs/",
                image_ref.registry, image_ref.namespace, image_ref.name
            );
            let blobs_dir = &format!("{}/{}/", mp.dir.clone(), "blobs-store",);
            log.info(&format!(
                "[release_mirror_to_disk] blobs_dir {}",
                blobs_dir.clone()
            ));
            let parsed_manifest = read_and_parse_oci_manifest(mf.file.clone())?;
            // not oci format
            let version = parsed_manifest.schema_version.unwrap();
            if version == 1 {
                let v1_mnfst = read_and_parse_manifest(mf.file.clone())?;
                let mut map: HashMap<String, Vec<FsLayer>> = HashMap::new();
                map.insert(blobs_url.to_string(), v1_mnfst.fs_layers.clone());
                execute_batch(
                    reg_con.clone(),
                    log,
                    blobs_dir.clone(),
                    mp.verify_blobs,
                    mp.tls_verify,
                    map,
                )
                .await?;
                vec_fslayer.append(&mut v1_mnfst.fs_layers.clone());
                log.info("[release_mirror_to_disk] completed release image index (v1) download");
            } else {
                let layers = parsed_manifest;
                log.debug(&format!("parsed manifest {:#?}", layers.clone()));
                // ignore config layer
                for layer in layers.clone().layers.unwrap().iter() {
                    let fslayer = FsLayer {
                        blob_sum: layer.digest.clone(),
                        original_ref: Some(mf.clone().original_ref),
                        size: Some(layer.size),
                        //number: None,
                    };
                    vec_fslayer.insert(0, fslayer);
                }
                let mut map: HashMap<String, Vec<FsLayer>> = HashMap::new();
                map.insert(blobs_url.to_string(), vec_fslayer.clone());
                execute_batch(
                    reg_con.clone(),
                    log,
                    blobs_dir.clone(),
                    mp.verify_blobs,
                    mp.tls_verify,
                    map,
                )
                .await?;
                log.info("[release_mirror_to_disk] completed release image index (v2) download");
            }
            let working_dir_cache = &format!(
                "{}/{}/{}/cache",
                mp.dir.clone(),
                image_ref.name.clone(),
                image_ref.version.clone(),
            );
            log.info(&format!("working_dir_cache {}", working_dir_cache));
            let cache_exists = Path::new(&working_dir_cache).exists();
            if cache_exists {
                rm_rf::remove(&working_dir_cache)
                    .expect("[release_mirror_to_disk] should delete current untarred cache");
            }
            let mut builder = DirBuilder::new();
            builder.mode(0o777);
            builder
                .create(&working_dir_cache)
                .expect("[release_mirror_to_disk] unable to create directory");
            untar_layers(
                log,
                blobs_dir.clone(),
                working_dir_cache.clone(),
                vec_fslayer.clone(),
            )
            .await;
            log.hi("[release_mirror_to_disk] completed untar of layers");
        }

        // find the directory 'release-manifests'
        // use the release image version (before the "-") to match
        let img = parse_image(log, release.name.clone());
        let version = img.version.clone();
        let mut arch = "all".to_string();
        if version.contains("-") {
            arch = img.version.split("-").nth(1).unwrap().to_string();
        }
        let base_dir = format!("{}/{}", mp.dir.clone(), "ocp-release");
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
                log.debug(&format!(
                    "[release_mirror_to_disk] check for release-references {} {} ",
                    version,
                    manifest_file.clone(),
                ));
                if manifest_file.contains(&version) && manifest_file.contains("image-references") {
                    let rm = parse_json_release_imagereference(
                        manifest_file.clone().to_string(),
                        mp.generic_override.clone(),
                    )?;
                    for img in rm.clone().spec.tags.iter() {
                        let image_ref = parse_image(log, img.clone().from.name);
                        if !(mp.skip_manifest_check == "release" || mp.skip_manifest_check == "all")
                            && !mp.dry_run
                        {
                            let manifest_url = &format!(
                                "https://{}/v2/{}/{}/manifests/{}",
                                image_ref.registry,
                                image_ref.namespace,
                                image_ref.name,
                                image_ref.version,
                            );
                            log.trace(&format!(
                                "[release_mirror_to_disk] manifest url {:#?}",
                                manifest_url.clone()
                            ));
                            // use the RegistryInterface to make the call
                            let res_manifest = reg_con
                                .get_manifest(manifest_url.clone(), token.clone())
                                .await?;
                            log.info(&format!(
                                "[release_mirror_to_disk] api call for manifest {:#?}",
                                img.name
                            ));
                            let f = &format!(
                                "{}/manifests/ocp-release/{}-{}.json",
                                mp.dir.clone(),
                                image_ref.version,
                                arch
                            );
                            let digest = get_sha_from_contents(res_manifest.clone().as_bytes());
                            if digest != image_ref.version.split(":").nth(1).unwrap() {
                                log.warn(&format!(
                                    "[release_mirror_to_disk] digest and file sha does not match {} : {}",
                                    img.name, image_ref.version
                                ));
                            }
                            process_and_update_manifest(
                                log,
                                res_manifest.clone(),
                                f.clone(),
                                mp.generic_override.clone(),
                            )
                            .await?;
                        }
                        let mnfst_on_disk = format!(
                            "{}/manifests/ocp-release/{}-{}.json",
                            mp.dir.clone(),
                            image_ref.version,
                            arch
                        );
                        // it may seem ridiculous to write then read from disk
                        // this was done intentionally as it ensures that our manifest
                        // file is correct and parsable
                        let local_manifest = read_and_parse_oci_manifest(mnfst_on_disk.clone())?;
                        log.debug(&format!(
                            "release_mirror_to_disk] checking manifest {}",
                            img.name
                        ));
                        log.debug(&format!(
                            "[release_mirror_to_disk] manifest on disk {} ",
                            mnfst_on_disk.clone()
                        ));
                        log.debug(&format!(
                            "[release_mirror_to_disk] sha {} ",
                            image_ref.version
                        ));
                        let op_url = format!(
                            "https://{}/v2/{}/{}/blobs/",
                            image_ref.registry, image_ref.namespace, image_ref.name,
                        );
                        for layer in local_manifest.clone().layers.unwrap().iter() {
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
                            };
                            vec_flayer.insert(0, fslayer);
                        }
                        // add configs
                        let config = local_manifest.config.unwrap();
                        let cfg = FsLayer {
                            blob_sum: config.digest,
                            original_ref: Some(img.from.name.clone()),
                            size: Some(config.size),
                        };
                        vec_flayer.insert(0, cfg);
                        // finally add the fslayers to the hashmap
                        fslayers.insert(op_url.clone(), vec_flayer.clone());
                        let mii = MirrorImageInfo {
                            reference: img.clone().from.name.clone(),
                            name: img.name.clone(),
                            arch: arch.to_string(),
                            //namespace: image_ref.namespace + &"/" + &image_ref.name,
                            namespace: "openshift/release".to_string(),
                            digest: image_ref.version,
                            manifest_type: "manifest".to_string(),
                            tag: Some(format!("{}-{}", version.clone(), img.name.clone())),
                            created: dt_formated.to_string(),
                            mirror_type: "ocp-release".to_string(),
                            bundle: None,
                        };
                        image_ref_tracker.insert(0, mii.clone());
                    }
                }
            }
        }
    }

    if releases.graph.is_some() && !mp.dry_run {
        if releases.graph.unwrap().contains("true") {
            log.info("[release_mirror_to_disk] build graph data");
            let g_impl = ImplGraphDataInterface {};
            let url = "https://api.openshift.com/api/upgrades_info/graph-data".to_string();
            g_impl.build_graph_image(log, mp.dir.clone(), url).await?;
            let p_fbi = process_fb_image(
                mp.dir.clone(),
                "graph-image".to_string(),
                "openshift".to_string(),
                "latest".to_string(),
                "ocp-release".to_string(),
            )
            .await?;
            image_ref_tracker.insert(0, p_fbi);
            g_impl.build_image_cleanup().await;
        }
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs_handler(
        mp.dir.clone() + &"/mirror-metadata/release-image-reference.json",
        "write",
        Some(serialized_manifest),
    )
    .await?;

    if mp.dry_run {
        let mut buf = String::from("");
        let md = read_and_parse_metadata(
            mp.dir.clone() + &"/mirror-metadata/release-image-reference.json",
        )?;
        for mii in md.iter() {
            if mii.arch == "amd64" || mii.arch == "x86_64" {
                let src = &format!("{}{}", "docker://", mii.reference);
                let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                buf = buf + &format!("{}={}\n", src, dest);
            }
        }
        fs_handler(
            mp.dir.clone() + "/mappings/release-mapping.txt",
            "write",
            Some(buf),
        )
        .await?;
        log.mid(&format!(
            "[release_mirror_to_disk] created release mapping file in folder {}",
            mp.dir.clone() + &"/mappings/",
        ));
    } else {
        let map = remove_duplicates(mp.dir.clone(), fslayers);
        let res = execute_batch(
            reg_con.clone(),
            log,
            format!("{}/{}", mp.dir.clone(), "blobs-store"),
            mp.verify_blobs,
            mp.tls_verify,
            map,
        )
        .await;
        if res.is_err() {
            return Err(res.err().unwrap());
        }
    }
    Ok(())
}

// utility functions section

pub fn parse_json_release_imagereference(
    file: String,
    map: HashMap<String, String>,
) -> Result<ReleaseSchema, MirrorError> {
    let res = fs::read_to_string(&file);
    if res.is_err() {
        let err = MirrorError::new(&format!(
            "[parse_json_release_imagereference] reading file {}",
            res.err().unwrap().to_string().to_lowercase()
        ));
        return Err(err);
    }
    // Parse the string of data into ReleaseSchema
    let res_parse = serde_json::from_str(&res.unwrap());
    if res_parse.is_err() {
        let err = MirrorError::new(&format!(
            "[parse_json_release_imagereference] parsing file {}",
            res_parse.err().unwrap().to_string().to_lowercase()
        ));
        return Err(err);
    }
    let res_value = map.get("registry-override");
    let mut root: ReleaseSchema = res_parse.unwrap();
    if res_value.is_some() {
        let value = res_value.unwrap();
        for img in root.spec.tags.iter_mut() {
            let from = img.from.name.split("/").nth(0).unwrap();
            if value.contains("http://") {
                img.from.name = img
                    .from
                    .name
                    .replace(from, value.split("http://").nth(1).unwrap());
            } else {
                img.from.name = img.from.name.replace(from, value);
            }
        }
    }
    Ok(root)
}

// get_sha_from_contents
pub fn get_sha_from_contents(manifest_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(manifest_bytes);
    let hash_bytes = hasher.finalize();
    encode(hash_bytes)
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
    use async_trait::async_trait;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    // do simple tests first
    #[test]
    fn parse_json_release_imagereference_should_fail() {
        let res = parse_json_release_imagereference("nada".to_string(), HashMap::new());
        assert_eq!(res.is_err(), true);
        fs::write("test.json", "{ stupid").expect("should write test file");
        let res_parse = parse_json_release_imagereference("test.json".to_string(), HashMap::new());
        assert_eq!(res_parse.is_err(), true);
        fs::remove_file("test.json").expect("should delete test.json");
    }

    #[test]
    fn release_mirror_to_disk_pass() {
        let _ = aw!(fs_handler(
            "test-artifacts/mirror-metadata".to_string(),
            "create_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/mappings".to_string(),
            "create_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/blobs-store/ac".to_string(),
            "create_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/manifests".to_string(),
            "create_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/artifacts".to_string(),
            "create_dir",
            None
        ));

        fs::copy(
            "test-artifacts/raw-tar-files/cincinnati-graph-data.tar.gz",
            "test-artifacts/artifacts/cincinnati-graph-data.tar.gz",
        )
        .expect("should copy tar.gz file");

        let from = "test-artifacts/raw-tar-files/ac/ac202bb709d9c0744e8fd6f3ed3c5c57eec4c7b16caeadac7b4b323f94f5809e".to_string();
        let to = "test-artifacts/blobs-store/ac/ac202bb709d9c0744e8fd6f3ed3c5c57eec4c7b16caeadac7b4b323f94f5809e".to_string();
        fs::copy(from, to).expect("should copy raw tar files");

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

        let log = &Logging {
            log_level: Level::INFO,
        };

        #[derive(Clone)]
        struct Fake {}

        #[async_trait]
        impl DownloadImageInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                let content = match url.clone() {
                    x if x.contains("test-release-image/manifests/v1.0") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-release-image/v1.0/manifest.json",
                        )
                        .expect("should read release-index manifest file");
                        content
                    }
                    x if x.contains("test-release-image/manifests/v2.0") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-release-image/v2.0/manifest.json",
                        )
                        .expect("should read release-index manifest file");
                        content
                    }
                    x if x.contains("test-release-image/manifests/multi") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-release-image/multi/manifest-list.json",
                        )
                        .expect("should read release-index manifest file");
                        content
                    }
                    x if x.contains("manifests/sha256:ade18f2994669ebeb870b3b545f8b48574da9fc26ea24341dd1c16faac9994a0") => {
                        let content = fs::read_to_string("test-artifacts/test-release-image/v1.0/release/test-simple-operator/manifest.json")
                        .expect("should read manifest file");
                        content
                    }
                    x if x.contains("error-release-image/manifests/latest") => {
                        let err = MirrorError::new("forced error");
                        return Err(err);
                    }
                    _ => "".to_string(),
                };
                Ok(content.to_string())
            }

            async fn get_blob(
                &self,
                log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _verify_blob: bool,
                _blob_sum: String,
            ) -> Result<(), MirrorError> {
                log.info("[get_blob] fake call");
                Ok(())
            }
        }

        let fake = Fake {};

        // setup mp struct for all sub-tests
        let mut map: HashMap<String, String> = HashMap::new();
        map.insert("registry-override".to_string(), url.clone());
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
            generic_override: map,
            rebuild_catalogs: Some(false),
        };
        let img = Image {
            name: format!(
                "{}/test/test-release-image:v2.0",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img = vec![img];
        let release = Release {
            graph: Some("false".to_string()),
            images: vec_img.clone(),
        };
        log.ex(&format!("executing sub test : v2 manifest [should pass]"));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);
        println!("");

        // skip manifest check
        mp.skip_manifest_check = "all".to_string();
        log.ex(&format!(
            "executing v2 manifest (skip manifest check) [should pass]"
        ));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release.clone(),
            mp.clone()
        ));
        assert_eq!(res.is_ok(), true);
        println!("");

        // use file override (manifest change)
        mp.skip_manifest_check = "none".to_string();
        mp.generic_override.insert(
            "./test-artifacts/ocp-release/v2.0/manifest.json".to_string(),
            "./test-artifacts/ocp-release/v2.0/override.json".to_string(),
        );
        log.ex(&format!(
            "executing sub test : v2 manifest (use file override) [should pass]"
        ));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);
        println!("");

        // use error image
        mp.skip_manifest_check = "none".to_string();
        let img_err = Image {
            name: format!(
                "{}/test/error-release-image:latest",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img_err = vec![img_err];
        let release_err = Release {
            graph: Some("false".to_string()),
            images: vec_img_err.clone(),
        };

        log.ex(&format!(
            "executing sub test : v2 manifest (forced error) [should fail]"
        ));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release_err.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_err(), true);
        println!("");

        // check version 1 manifest with graph set true
        mp.generic_override.insert(
            "./test-artifacts/ocp-release/v1.0/manifest.json".to_string(),
            "./test-artifacts/ocp-release/v1.0/override.json".to_string(),
        );
        let img_v1 = Image {
            name: format!(
                "{}/test/test-release-image:v1.0",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img_v1 = vec![img_v1];
        let release_v1 = Release {
            graph: Some("true".to_string()),
            images: vec_img_v1,
        };
        log.ex(&format!("executing sub test : v1 manifest [should pass]"));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release_v1.clone(),
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);
        println!("");

        // check dry run
        mp.dry_run = true;
        mp.generic_override = HashMap::new();
        let img_dr = Image {
            name: format!(
                "{}/test/test-release-image:v1.0",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img_dr = vec![img_dr];
        let release_dr = Release {
            graph: Some("false".to_string()),
            images: vec_img_dr,
        };
        log.ex(&format!("executing sub test : dry-run [should pass]"));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release_dr.clone(),
            mp.clone()
        ));
        assert_eq!(res.is_ok(), true);
        println!("");

        // test multi image
        mp.dry_run = false;
        mp.generic_override = HashMap::new();
        mp.skip_manifest_check = "none".to_string();
        let img_multi = Image {
            name: format!(
                "{}/test/test-release-image:multi",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img_multi = vec![img_multi];
        let release_multi = Release {
            graph: Some("false".to_string()),
            images: vec_img_multi,
        };
        log.ex(&format!("executing sub test : multi image [should pass]"));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release_multi,
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);

        // simulate a file change
        mp.generic_override.insert(
            "./test-artifacts/ocp-release/multi-s390x/manifest.json".to_string(),
            "./test-artifacts/ocp-release/multi-s390x/override.json".to_string(),
        );
        mp.dry_run = false;
        mp.skip_manifest_check = "none".to_string();
        let img_multi = Image {
            name: format!(
                "{}/test/test-release-image:multi",
                url.split("http://").nth(1).unwrap()
            ),
        };
        let vec_img_multi = vec![img_multi];
        let release_multi = Release {
            graph: Some("false".to_string()),
            images: vec_img_multi,
        };
        log.ex(&format!(
            "executing sub test : multi image (file override) [should pass]"
        ));
        let res = aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            release_multi,
            mp.clone()
        ));
        if res.is_err() {
            log.error(&format!(
                "result -> {:?}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_ok(), true);

        let _ = aw!(fs_handler(
            "test-artifacts/mirror-metadata".to_string(),
            "remove_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/mappings".to_string(),
            "remove_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/blobs-store/ac".to_string(),
            "remove_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/manifests".to_string(),
            "remove_dir",
            None
        ));
        let _ = aw!(fs_handler(
            "test-artifacts/artifacts".to_string(),
            "remove_dir",
            None
        ));
    }
}
