use flate2::read::GzDecoder;
use futures::{stream, StreamExt};
use reqwest::Client;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::path::Path;
use tar::Archive;

use crate::api::schema::*;
use crate::log::logging::*;

// get manifest async api call
pub async fn get_manifest(
    url: String,
    token: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut header_bearer: String = "Bearer ".to_owned();
    header_bearer.push_str(&token);
    let body = client
        .get(url)
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .header("Content-Type", "application/json")
        .header("Authorization", header_bearer)
        .send()
        .await?
        .text()
        .await?;
    Ok(body)
}

// get each blob referred to by the vector in parallel
// set by the PARALLEL_REQUESTS value
pub async fn get_blobs(log: &Logging, url: String, token: String, layers: Vec<FsLayer>) {
    const PARALLEL_REQUESTS: usize = 8;

    let client = Client::new();
    let mut header_bearer: String = "Bearer ".to_owned();
    header_bearer.push_str(&token);

    // remove all duplicates in FsLayer
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for img in layers.iter() {
        // truncate sha256:
        let truncated_image = img.blob_sum.split(":").nth(1).unwrap();
        let inner_blobs_file = get_blobs_file(&truncated_image);
        let exist = Path::new(&inner_blobs_file).exists();
        if !seen.contains(truncated_image) && !exist {
            seen.insert(truncated_image);
            images.push(img.blob_sum.clone());
        }
    }

    let fetches = stream::iter(images.into_iter().map(|blob| {
        let client = client.clone();
        let url = url.clone();
        let header_bearer = header_bearer.clone();
        async move {
            match client
                .get(url + &blob)
                .header("Authorization", header_bearer)
                .send()
                .await
            {
                Ok(resp) => match resp.bytes().await {
                    Ok(bytes) => {
                        let blob = blob.split(":").nth(1).unwrap();
                        let blob_dir = get_blobs_dir(blob);
                        fs::create_dir_all(blob_dir.clone()).expect("unable to create direcory");
                        fs::write(blob_dir + &blob, bytes.clone()).expect("unable to write blob");
                        let msg = format!("writing blob {}", blob);
                        log.info(&msg);
                    }
                    Err(_) => {
                        let msg = format!("reading blob {}", &blob);
                        log.error(&msg);
                    }
                },
                Err(_) => {
                    let msg = format!("downloading blob {}", &blob);
                    log.error(&msg);
                }
            }
        }
    }))
    .buffer_unordered(PARALLEL_REQUESTS)
    .collect::<Vec<()>>();
    log.info("downloading blobs...");
    fetches.await;
}

// untar layers in directory denoted by parameter 'dir'
pub async fn untar_layers(log: &Logging, dir: String, layers: Vec<FsLayer>) {
    // clean all duplicates
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for img in layers.iter() {
        // truncate sha256:
        let truncated_image = img.blob_sum.split(":").nth(1).unwrap();
        if !seen.contains(truncated_image) {
            seen.insert(truncated_image);
            images.push(img.blob_sum.clone());
        }
    }

    // read directory, iterate each file and untar
    for path in images.iter() {
        let blob = path.split(":").nth(1).unwrap();
        let cache_file = dir.clone() + "/" + &blob[..6];
        if !Path::new(&cache_file).exists() {
            let file = get_blobs_file(blob);
            let tar_gz = File::open(file).expect("could not open file");
            let tar = GzDecoder::new(tar_gz);
            let mut archive = Archive::new(tar);
            // should always be a sha256 string
            log.info(&format!("untarring file {} ", &blob[..6]));
            // we are really interested in either the configs or release-images directories
            match archive.unpack(cache_file) {
                Ok(arch) => arch,
                Err(error) => {
                    let msg = format!("skipping this error : {} ", &error.to_string());
                    log.warn(&msg);
                }
            };
        } else {
            log.debug(&format!("cache exists {}", cache_file));
        }
    }
}

// parse_image_index - best attempt to parse image index and return catalog reference
pub fn parse_image_index(log: &Logging, operators: Option<Vec<Operator>>) -> Vec<ImageReference> {
    let mut image_refs = vec![];
    let mut pkgs = vec![];
    for ops in operators.unwrap().iter() {
        let img = ops.catalog.clone();
        log.trace(&format!("LMZ catalogs {:#?}", ops.catalog));
        let mut i = img.split(":");
        let index = i.nth(0).unwrap();
        let mut hld = index.split("/");
        let ver = i.nth(0).unwrap();
        for pkg in ops.packages.clone().unwrap().iter() {
            pkgs.insert(0, pkg.name.clone());
        }
        let ir = ImageReference {
            registry: hld.nth(0).unwrap().to_string(),
            namespace: hld.nth(0).unwrap().to_string(),
            name: hld.nth(0).unwrap().to_string(),
            version: ver.to_string(),
            packages: pkgs.clone(),
        };

        log.debug(&format!("image reference {:#?}", img));
        image_refs.insert(0, ir);
    }
    image_refs
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

// construct the operator namespace and name
pub fn get_operator_name(img: String) -> String {
    let mut parts = img.split("/");
    let _ = parts.nth(0).unwrap();
    let ns = parts.nth(0).unwrap();
    let name = parts.nth(0).unwrap();
    let mut op_name = name.split("@");
    ns.to_string() + "/" + &op_name.nth(0).unwrap().to_owned()
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

// contruct a manifest url from a string by digest
pub fn get_manifest_url_by_digest(url: String, digest: String) -> String {
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
    url.push_str(&digest);
    url
}

// construct the blobs url
pub fn get_blobs_url(image_ref: ImageReference) -> String {
    // return a string in the form of (example below)
    // "https://registry.redhat.io/v2/redhat/certified-operator-index/blobs/";
    let mut url = String::from("https://");
    url.push_str(&image_ref.registry);
    url.push_str(&"/v2/");
    url.push_str(&image_ref.namespace);
    url.push_str("/");
    url.push_str(&image_ref.name);
    url.push_str(&"/");
    url.push_str(&"blobs/");
    url
}

// construct the blobs url by string
pub fn get_blobs_url_by_string(img: String) -> String {
    let mut parts = img.split("/");
    let mut url = String::from("https://");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/v2/");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/");
    let i = parts.nth(0).unwrap();
    let mut sha = i.split("@");
    url.push_str(&sha.nth(0).unwrap());
    url.push_str(&"/blobs/");
    url
}

// construct blobs dir
fn get_blobs_dir(name: &str) -> String {
    let mut file = String::from("working-dir/blobs-store/");
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file
}

// construct blobs file
fn get_blobs_file(name: &str) -> String {
    let mut file = String::from("working-dir/blobs-store/");
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file.push_str(&name);
    file
}
