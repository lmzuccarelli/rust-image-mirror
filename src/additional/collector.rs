use crate::api::schema::*;
use crate::auth::credentials::*;
use crate::batch::copy::*;
use crate::index::resolve::*;
use crate::log::logging::*;

use std::fs;
use std::path::Path;
use walkdir::WalkDir;

// collect all additional images
pub async fn additional_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    additional: Vec<Image>,
) {
    log.hi("additional image collector mode: mirrorToDisk");
    for img in additional.iter() {
        let additional_dir = get_dir_from_isc(img.name.clone());
        log.info(&format!("additional dir {}", additional_dir));
        fs::create_dir_all(dir.clone() + &"/" + &additional_dir.clone())
            .expect("should create additional images dir");
        let manifest_url = get_manifest_url(img.name.clone());
        log.info(&format!("manifest url {}", manifest_url));
        let img_ref = get_image_reference(&img.name);
        let token = get_token(log, img_ref.clone().registry).await;
        log.trace(&format!("manifest url {:#?}", manifest_url.clone()));
        let manifest_json = get_manifest_json_file(
            // ./working-dir
            dir.clone(),
            img_ref.name.clone(),
            img_ref.version.clone(),
        );

        // use the RegistryInterface to make the call
        let manifest = reg_con
            .get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        log.info(&format!("checking manifest for {:#?}", img.name.clone()));
        log.debug(&format!("manifest {:#?}", manifest.clone()));
        let _working_dir_cache =
            get_cache_dir(dir.clone(), img_ref.name.clone(), img_ref.version.clone());
        let manifest_exists = Path::new(&manifest_json).exists();
        let res_manifest_in_mem = parse_json_manifest_index(manifest.clone()).unwrap();
        let sub_dir = dir.clone() + "/blobs-store/";
        let mut exists = true;
        if manifest_exists {
            let manifest_on_disk = fs::read_to_string(&manifest_json).unwrap();
            let res_manifest_on_disk = parse_json_manifest_index(manifest_on_disk).unwrap();
            if res_manifest_on_disk != res_manifest_in_mem {
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
        }
    }
}

pub async fn additional_disk_to_mirror<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    destination_url: String,
    additional: Vec<Image>,
) -> String {
    // read isc additional images
    // read all manifests and blobs from disk
    // build the list
    // call push_image
    log.hi("additional images collector mode: diskToMirror");
    for img in additional.iter() {
        log.info(&format!("additional {:#?} ", &img.name));
        let additional_dir = dir.clone() + &get_dir_from_isc(img.name.clone());
        log.debug(&format!("addtional directory {}", additional_dir.clone()));
        let manifests = get_all_assosciated_manifests(log, additional_dir.clone());
        // using map and collect are not async
        for mm in manifests.iter() {
            let manifest = get_manifest(mm.to_string());
            let _res = reg_con
                .push_image(
                    log,
                    "./working-dir/".to_string(),
                    additional_dir.clone(),
                    destination_url.clone(),
                    String::from(""),
                    manifest.clone(),
                )
                .await;
        }
    }
    "ok".to_string()
}

fn get_dir_from_isc(additional: String) -> String {
    let res = additional.split("/");
    let collection = res.clone().collect::<Vec<&str>>();
    let name = collection[2].split(":");
    let result = "/additional/".to_string()
        + name.clone().nth(0).unwrap()
        + "/"
        + name.clone().nth(1).unwrap();
    result
}

fn get_all_assosciated_manifests(log: &Logging, dir: String) -> Vec<String> {
    let mut vec_manifests: Vec<String> = vec![];
    let result = WalkDir::new(&dir);
    for file in result.into_iter().filter_map(|file| file.ok()) {
        if file.metadata().unwrap().is_file() & !file.path().display().to_string().contains("list")
        {
            log.trace(&format!(
                "assosciated manifest found {:#?}",
                file.path().display().to_string()
            ));
            vec_manifests.insert(0, file.path().display().to_string());
        }
    }
    vec_manifests
}

fn get_manifest_url(url: String) -> String {
    let mut parts = url.split("/");
    let mut url = String::from("https://");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/v2/");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/");
    let i = parts.nth(0).unwrap();
    if i.contains("@") {
        let mut sha = i.split("@");
        url.push_str(&sha.nth(0).unwrap());
        url.push_str(&"/");
        url.push_str(&"manifests/");
        url.push_str(&sha.nth(0).unwrap());
    } else {
        let mut tag = i.split(":");
        url.push_str(tag.nth(0).unwrap());
        url.push_str(&"/manifests/");
        url.push_str(&tag.nth(0).unwrap());
    }
    url
}

// utility functions - get_manifest_json
pub fn get_manifest_json_file(dir: String, name: String, version: String) -> String {
    let mut file = dir.clone();
    file.push_str(&"/additional/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/");
    file.push_str(&"manifest.json");
    file
}

fn get_image_reference(reg: &str) -> ImageReference {
    let mut hld = reg.split("/");
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
        packages: None,
    };
    ir
}

// parse the manifest json for additional indexes only
pub fn parse_json_manifest_index(
    data: String,
) -> Result<ManifestSchema, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::ManifestSchema.
    let root: ManifestSchema = serde_json::from_str(&data)?;
    Ok(root)
}

// parse the manifest json for additional
pub fn parse_json_manifest(data: String) -> Result<Manifest, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: Manifest = serde_json::from_str(&data)?;
    Ok(root)
}

fn get_manifest(dir: String) -> Manifest {
    let data = fs::read_to_string(&dir).expect("should read various arch manifest files");
    let manifest = parse_json_manifest(data).unwrap();
    manifest
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    //use super::*;
    //use crate::error::handler::MirrorError;
    //use async_trait::async_trait;

    //macro_rules! aw {
    //    ($e:expr) => {
    //        tokio_test::block_on($e)
    //    };
    //}
}
