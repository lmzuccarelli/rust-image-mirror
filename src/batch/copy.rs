// module copy

use futures::{stream, StreamExt};
use reqwest::Client;
use std::collections::HashSet;
use std::fs;
//use std::fs::File;
use std::path::Path;
//use tar::Archive;

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
            if url == "" {
                let img_orig = img.original_ref.clone().unwrap();
                let img_ref = get_blobs_url_by_string(img_orig);
                let layer = FsLayer {
                    blob_sum: img.blob_sum.clone(),
                    original_ref: Some(img_ref),
                };
                images.push(layer);
            } else {
                let layer = FsLayer {
                    blob_sum: img.blob_sum.clone(),
                    original_ref: Some(url.clone()),
                };
                images.push(layer);
            }
        }
    }

    let fetches = stream::iter(images.into_iter().map(|blob| {
        let client = client.clone();
        let url = blob.original_ref.unwrap().clone();
        let header_bearer = header_bearer.clone();
        async move {
            match client
                .get(url + &blob.blob_sum)
                .header("Authorization", header_bearer)
                .send()
                .await
            {
                Ok(resp) => match resp.bytes().await {
                    Ok(bytes) => {
                        let blob = blob.blob_sum.split(":").nth(1).unwrap();
                        let blob_dir = get_blobs_dir(blob);
                        fs::create_dir_all(blob_dir.clone()).expect("unable to create direcory");
                        fs::write(blob_dir + &blob, bytes.clone()).expect("unable to write blob");
                        let msg = format!("writing blob {}", blob);
                        log.info(&msg);
                    }
                    Err(_) => {
                        let msg = format!("reading blob {}", &blob.blob_sum);
                        log.error(&msg);
                    }
                },
                Err(_) => {
                    let msg = format!("downloading blob {}", &blob.blob_sum);
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
pub fn get_blobs_dir(name: &str) -> String {
    let mut file = String::from("working-dir/blobs-store/");
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file
}

// construct blobs file
pub fn get_blobs_file(name: &str) -> String {
    let mut file = String::from("working-dir/blobs-store/");
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file.push_str(&name);
    file
}
