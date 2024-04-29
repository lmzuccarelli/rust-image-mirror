use custom_logger::*;
use mirror_auth::*;
use mirror_catalog_index::*;
use mirror_copy::*;
use serde_derive::{Deserialize, Serialize};
use std::fs;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use walkdir::WalkDir;

use crate::config::load::*;

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

// collect all operator images
pub async fn release_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    releases: Vec<Release>,
) {
    log.hi("release collector mode: mirrorToDisk");

    // parse the config
    for release in releases.iter() {
        let img_ref = convert_release_image_index(log, release.image.clone());
        log.debug(&format!("image refs {:#?}", img_ref));

        let manifest_json =
            get_manifest_json_file(dir.clone(), img_ref.name.clone(), img_ref.version.clone());
        log.trace(&format!("manifest json file {}", manifest_json));
        let token = get_token(log, img_ref.clone().registry).await;
        let manifest_url = get_image_manifest_url(img_ref.clone());
        log.trace(&format!("manifest url {}", manifest_url));
        let manifest = reg_con
            .get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        let manifest_dir = manifest_json.split("manifest.json").nth(0).unwrap();
        log.info(&format!("manifest directory {}", manifest_dir));
        fs::create_dir_all(manifest_dir).expect("unable to create directory manifest directory");
        let manifest_exists = Path::new(&manifest_json).exists();
        let res_manifest_in_mem = parse_json_manifest(manifest.clone()).unwrap();
        let working_dir_cache =
            get_cache_dir(dir.clone(), img_ref.name.clone(), img_ref.version.clone());
        let cache_exists = Path::new(&working_dir_cache).exists();
        let sub_dir = dir.clone() + "blobs-store/";
        log.hi(&format!("working dir cache {} ", working_dir_cache));
        log.hi(&format!("sub_dir {} ", sub_dir.clone()));
        let mut exists = true;
        if manifest_exists {
            let manifest_on_disk = fs::read_to_string(&manifest_json).unwrap();
            let res_manifest_on_disk = parse_json_manifest(manifest_on_disk).unwrap();
            if res_manifest_on_disk != res_manifest_in_mem || !cache_exists {
                exists = false;
            }
        } else {
            exists = false;
        }
        if !exists {
            log.info("detected change in index manifest");
            fs::write(manifest_json, manifest.clone())
                .expect("unable to write (index) manifest.json file");
            let blobs_url = get_blobs_url(img_ref.clone());
            // use a concurrent process to get related blobs
            let response = reg_con
                .get_blobs(
                    log,
                    sub_dir.clone(),
                    blobs_url,
                    token.clone(),
                    res_manifest_in_mem.fs_layers.clone(),
                )
                .await;
            log.info(&format!(
                "completed release image index download {:#?}",
                response
            ));
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
                sub_dir.clone(),
                working_dir_cache.clone(),
                res_manifest_in_mem.fs_layers,
            )
            .await;
            log.hi("completed untar of layers");
        }

        // find the directory 'release-manifests'
        let config_dir = find_dir(
            log,
            working_dir_cache.clone(),
            "release-manifests".to_string(),
        )
        .await;
        log.mid(&format!(
            "full path for directory 'release-manifests' {} ",
            &config_dir
        ));

        // parse the image-references json from release-manfests directory
        let imgs = parse_json_release_imagereference(config_dir + "/image-references");
        log.trace(&format!(
            "images from release-manifests/image-reference {:#?}",
            imgs
        ));

        // iterate through all the release image-references
        let release_dir =
            dir.clone() + "/" + &img_ref.clone().name + "/" + &img_ref.clone().version + "/";
        for img in imgs.unwrap().spec.tags.iter() {
            // first check if the release operators exist on disk
            let release_op_dir = release_dir.clone() + "/release/" + &img.name;
            fs::create_dir_all(release_op_dir.clone()).expect("should create release operator dir");
            let manifest_url = get_manifest_url(img.from.name.clone());
            log.trace(&format!("manifest url {:#?}", manifest_url.clone()));
            // use the RegistryInterface to make the call
            let manifest = reg_con
                .get_manifest(manifest_url.clone(), token.clone())
                .await
                .unwrap();

            log.info(&format!("checking manifest for {:#?}", img.name.clone()));
            log.trace(&format!("manifest contents {:#?}", manifest));
            let release_op = release_op_dir.clone() + "/manifest.json";
            let metadata = fs::metadata(release_op);
            // TODO: surely there is a better way ;)
            if metadata.is_ok() {
                let meta = metadata.as_ref().unwrap();
                if meta.len() != manifest.len() as u64 {
                    log.info(&format!("writing manifest for {:#?}", img.name.clone()));
                    fs::write(release_op_dir + "/manifest.json", manifest.clone())
                        .expect("unable to write manifest.json file");
                }
            } else if metadata.is_err() {
                log.info(&format!("writing manifest for {:#?}", img.name.clone()));
                fs::write(release_op_dir + "/manifest.json", manifest.clone())
                    .expect("unable to write manifest.json file");
            }
            let mut fslayers = Vec::new();
            let op_manifest = parse_json_manifest_operator(manifest.clone()).unwrap();
            let origin_tmp = img.from.name.split("@");
            let origin = origin_tmp.clone().nth(0).unwrap();

            // convert op_manifest.layer to FsLayer
            for layer in op_manifest.layers.unwrap().iter() {
                let fslayer = FsLayer {
                    blob_sum: layer.digest.clone(),
                    original_ref: Some(origin.to_string()),
                    size: Some(layer.size),
                };
                fslayers.insert(0, fslayer);
            }
            // add configs
            let config = op_manifest.config.unwrap();
            let cfg = FsLayer {
                blob_sum: config.digest,
                original_ref: Some(origin.to_string()),
                size: Some(config.size),
            };
            fslayers.insert(0, cfg);
            let op_url = get_blobs_url_by_string(img.from.name.clone());
            let blobs_dir = dir.clone() + &"/blobs-store/".to_string();
            log.trace(&format!("blobs_url {}", op_url));
            log.trace(&format!("fslayer for {} {:#?}", img.name, fslayers));

            let _res = reg_con
                .get_blobs(
                    log,
                    blobs_dir.clone(),
                    op_url,
                    token.clone(),
                    fslayers.clone(),
                )
                .await;
        }
    }
}

pub async fn release_disk_to_mirror<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    destination_url: String,
    releases: Vec<Release>,
) -> String {
    for release in releases {
        let release_dir = dir.clone() + &get_dir_from_isc(release.image.clone());
        log.debug(&format!("release directory {}", release_dir.clone()));
        let manifests = get_all_assosciated_manifests(log, release_dir);
        // using map and collect are not async
        for mm in manifests.iter() {
            // we can infer some info from the manifest
            let binding = mm.to_string();
            let manifest = get_release_manifest(binding.clone());
            log.trace(&format!("manifest struct {:#?}", manifest));
            log.trace(&format!("directory {}", binding));
            let _res = reg_con
                .push_image(
                    log,
                    dir.clone(),
                    String::from("ocp-release"),
                    destination_url.clone(),
                    String::from(""),
                    manifest.clone(),
                )
                .await;
        }
    }
    String::from("ok")
}

// utility functions

pub fn parse_json_release_imagereference(
    file: String,
) -> Result<ReleaseSchema, Box<dyn std::error::Error>> {
    let data = fs::read_to_string(&file)
        .expect("should read release-manifests/image-references json file");
    // Parse the string of data into ReleaseSchema
    let root: ReleaseSchema = serde_json::from_str(&data)?;
    Ok(root)
}

fn get_dir_from_isc(release: String) -> String {
    let res = release.split("/");
    let collection = res.clone().collect::<Vec<&str>>();
    let name = collection[2].split(":");
    let result = name.clone().nth(0).unwrap().to_string()
        + "/"
        + name.clone().nth(1).unwrap()
        + &"/release/";
    result
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

// contruct the manifest url
pub fn get_image_manifest_url(image_ref: ImageReference) -> String {
    // return a string in the form of (example below)
    // "https://registry.redhat.io/v2/redhat/certified-operator-index/manifests/v4.12";
    let mut url = String::from("https://");
    url.push_str(&image_ref.registry);
    url.push_str(&"/v2/");
    url.push_str(&image_ref.namespace);
    url.push_str(&"/");
    url.push_str(&image_ref.name);
    url.push_str(&"/");
    url.push_str(&"manifests/");
    url.push_str(&image_ref.version);
    url
}

// contruct a manifest url from a string
pub fn get_manifest_url(url: String) -> String {
    let mut parts = url.split("/");
    let mut url = String::from("https://");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/v2/");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/");
    let i = parts.nth(0).unwrap();
    let mut sha = i.split("@");
    url.push_str(&sha.nth(0).unwrap());
    url.push_str(&"/");
    url.push_str(&"manifests/");
    url.push_str(&sha.nth(0).unwrap());
    url
}

pub fn parse_json_manifest_operator(data: String) -> Result<Manifest, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: Manifest = serde_json::from_str(&data)?;
    Ok(root)
}

pub fn get_all_assosciated_manifests(log: &Logging, dir: String) -> Vec<String> {
    let mut vec_manifests: Vec<String> = vec![];
    let result = WalkDir::new(&dir);
    for file in result.into_iter().filter_map(|file| file.ok()) {
        if file.metadata().unwrap().is_file() & !file.path().display().to_string().contains("list")
        {
            log.debug(&format!(
                "assosciated manifest found {:#?}",
                file.path().display().to_string()
            ));
            vec_manifests.insert(0, file.path().display().to_string());
        }
    }
    vec_manifests
}

fn get_release_manifest(dir: String) -> Manifest {
    let data = fs::read_to_string(&dir).expect("should read release-operator-manifest json file");
    let release_manifest = parse_json_manifest_operator(data).unwrap();
    release_manifest
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

        #[derive(Clone)]
        struct Fake {}
    }
}
