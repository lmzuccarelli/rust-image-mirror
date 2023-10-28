// module copy

use async_trait::async_trait;
use futures::{stream, StreamExt};
use reqwest::Client;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::api::schema::*;
use crate::log::logging::*;

#[async_trait]
impl RegistryInterface for ImplRegistryInterface {
    async fn get_manifest(
        &self,
        url: String,
        token: String,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let client = Client::new();
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
    async fn get_blobs(
        &self,
        log: &Logging,
        dir: String,
        url: String,
        token: String,
        layers: Vec<FsLayer>,
    ) -> String {
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
            let inner_blobs_file = get_blobs_file(dir.clone(), &truncated_image);
            let exist = Path::new(&inner_blobs_file).exists();
            if !seen.contains(truncated_image) && !exist {
                seen.insert(truncated_image);
                if url == "" {
                    let img_orig = img.original_ref.clone().unwrap();
                    let img_ref = get_blobs_url_by_string(img_orig);
                    let layer = FsLayer {
                        blob_sum: img.blob_sum.clone(),
                        original_ref: Some(img_ref),
                        result: Some(String::from("")),
                    };
                    images.push(layer);
                } else {
                    let layer = FsLayer {
                        blob_sum: img.blob_sum.clone(),
                        original_ref: Some(url.clone()),
                        result: Some(String::from("")),
                    };
                    images.push(layer);
                }
            }
        }
        log.trace(&format!("fslayers vector {:#?}", images));
        let fetches = stream::iter(images.into_iter().map(|blob| {
            let client = client.clone();
            let url = blob.original_ref.unwrap().clone();
            let header_bearer = header_bearer.clone();
            let wrk_dir = dir.clone();
            async move {
                match client
                    .get(url.clone() + &blob.blob_sum)
                    .header("Authorization", header_bearer)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.bytes().await {
                        Ok(bytes) => {
                            let blob_digest = blob.blob_sum.split(":").nth(1).unwrap();
                            let blob_dir = get_blobs_dir(wrk_dir.clone(), blob_digest);
                            fs::create_dir_all(blob_dir.clone())
                                .expect("unable to create direcory");
                            fs::write(blob_dir + &blob_digest, bytes.clone())
                                .expect("unable to write blob");
                            let msg = format!("writing blob {}", blob_digest);
                            log.info(&msg);
                        }
                        Err(_) => {
                            let msg = format!("reading blob {}", url.clone());
                            log.error(&msg);
                        }
                    },
                    Err(_) => {
                        let msg = format!("downloading blob {}", &url);
                        log.error(&msg);
                    }
                }
            }
        }))
        .buffer_unordered(PARALLEL_REQUESTS)
        .collect::<Vec<()>>();
        log.debug("downloading blobs...");
        fetches.await;
        String::from("ok")
    }
    // push each blob referred to by the vector in parallel
    // set by the PARALLEL_REQUESTS value
    async fn push_blobs(
        &self,
        log: &Logging,
        dir: String,
        url: String,
        token: String,
        layers: Vec<FsLayer>,
    ) -> String {
        // 1. first step is to post a blob
        // POST /v2/<name>/blobs/uploads/
        // if the POST request is successful, a 202 Accepted response will be returned with the upload URL in the Location header:
        // 202 Accepted
        // Location: /v2/<name>/blobs/uploads/<uuid>
        // Range: bytes=0-<offset>
        // Content-Length: 0
        // Docker-Upload-UUID: <uuid>
        //
        // 2. check if the blob exists
        // HEAD /v2/<name>/blobs/<digest>
        // If the layer with the digest specified in digest is available, a 200 OK response will be received,
        // with no actual body content (this is according to http specification). The response will look as follows:
        // 200 OK
        // Content-Length: <length of blob>
        // Docker-Content-Digest: <digest>
        //
        // 3. if it does not exist do a put
        // PUT /v2/<name>/blobs/uploads/<uuid>?digest=<digest>
        // Content-Length: <size of layer>
        // Content-Type: application/octet-stream
        // 201 Created
        // Location: /v2/<name>/blobs/<digest>
        // Content-Length: 0
        // Docker-Content-Digest: <digest>
        //
        // continue for each blob in the specifid container
        //
        // 4. Upload the manifest
        // PUT /v2/<name>/manifests/<reference>
        // Content-Type: <manifest media type>
        /*
          {
            "name": <name>,
            "tag": <tag>,
            "fsLayers": [
                {
                    "blobSum": <digest>
                },
                ...
            ],
            "history": <v1 images>,
            "signature": <JWS>,
            ...
            }
        */

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
            let inner_blobs_file = get_blobs_file(dir.clone(), &truncated_image);
            let exist = Path::new(&inner_blobs_file).exists();
            if !seen.contains(truncated_image) && !exist {
                seen.insert(truncated_image);
                if url == "" {
                    let img_orig = img.original_ref.clone().unwrap();
                    let img_ref = get_blobs_url_by_string(img_orig);
                    let layer = FsLayer {
                        blob_sum: img.blob_sum.clone(),
                        original_ref: Some(img_ref),
                        result: Some(String::from("")),
                    };
                    images.push(layer);
                } else {
                    let layer = FsLayer {
                        blob_sum: img.blob_sum.clone(),
                        original_ref: Some(url.clone()),
                        result: Some(String::from("")),
                    };
                    images.push(layer);
                }
            }
        }
        log.trace(&format!("fslayers vector {:#?}", images));
        let fetches = stream::iter(images.into_iter().map(|blob| {
            let client = client.clone();
            let url = blob.original_ref.unwrap().clone();
            let header_bearer = header_bearer.clone();
            let wrk_dir = dir.clone();
            async move {
                match client
                    .get(url.clone() + &blob.blob_sum)
                    .header("Authorization", header_bearer)
                    .send()
                    .await
                {
                    Ok(resp) => match resp.bytes().await {
                        Ok(bytes) => {
                            let blob_digest = blob.blob_sum.split(":").nth(1).unwrap();
                            let blob_dir = get_blobs_dir(wrk_dir.clone(), blob_digest);
                            fs::create_dir_all(blob_dir.clone())
                                .expect("unable to create blob directory");
                            fs::write(blob_dir + &blob_digest, bytes.clone())
                                .expect("unable to write blob");
                            let msg = format!("writing blob {}", blob_digest);
                            log.info(&msg);
                        }
                        Err(_) => {
                            let msg = format!("reading blob {}", url.clone());
                            log.error(&msg);
                        }
                    },
                    Err(_) => {
                        let msg = format!("downloading blob {}", &url);
                        log.error(&msg);
                    }
                }
            }
        }))
        .buffer_unordered(PARALLEL_REQUESTS)
        .collect::<Vec<()>>();
        log.debug("pushing blobs...");
        fetches.await;
        String::from("ok")
    }
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
pub fn get_blobs_dir(dir: String, name: &str) -> String {
    // originally working-dir/blobs-store
    let mut file = dir.clone();
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file
}
// construct blobs file
pub fn get_blobs_file(dir: String, name: &str) -> String {
    // originally working-dir/blobs-store
    let mut file = dir.clone();
    file.push_str("/");
    file.push_str(&name[..2]);
    file.push_str(&"/");
    file.push_str(&name);
    file
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn get_manifest_pass() {
        let mut server = mockito::Server::new();
        let url = server.url();

        // Create a mock
        server
            .mock("GET", "/manifests")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{ \"test\": \"hello-world\" }")
            .create();

        let real = ImplRegistryInterface {};

        let res = aw!(real.get_manifest(url + "/manifests", String::from("token")));
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), String::from("{ \"test\": \"hello-world\" }"));
    }

    #[test]
    fn get_blobs_pass() {
        let mut server = mockito::Server::new();
        let url = server.url();

        // Create a mock
        server
            .mock("GET", "/sha256:1234567890")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{ \"test\": \"hello-world\" }")
            .create();

        let fslayer = FsLayer {
            blob_sum: String::from("sha256:1234567890"),
            original_ref: Some(url.clone()),
            result: Some(String::from("")),
        };
        let fslayers = vec![fslayer];
        let log = &Logging {
            log_level: Level::INFO,
        };

        let real = ImplRegistryInterface {};

        // test with url set first
        aw!(real.get_blobs(
            log,
            String::from("test-artifacts/test-blobs-store/"),
            url.clone() + "/",
            String::from("token"),
            fslayers.clone(),
        ));
        // check the file contents
        let s = fs::read_to_string("test-artifacts/test-blobs-store/12/1234567890")
            .expect("should read file");
        assert_eq!(s, "{ \"test\": \"hello-world\" }");
        fs::remove_dir_all("test-artifacts/test-blobs-store").expect("should delete");
    }

    #[test]
    fn get_blobs_file_pass() {
        let res = get_blobs_file(
            String::from("test-artifacts/index-manifest/v1/blobs-store"),
            "1234567890",
        );
        assert_eq!(
            res,
            String::from("test-artifacts/index-manifest/v1/blobs-store/12/1234567890")
        );
    }

    #[test]
    fn get_blobs_dir_pass() {
        let res = get_blobs_dir(
            String::from("test-artifacts/index-manifest/v1/blobs-store/"),
            "1234567890",
        );
        assert_eq!(
            res,
            String::from("test-artifacts/index-manifest/v1/blobs-store/12/")
        );
    }

    #[test]
    fn get_blobs_url_by_string_pass() {
        let res = get_blobs_url_by_string(String::from(
            "test.registry.io/test/some-operator@sha256:1234567890",
        ));
        assert_eq!(
            res,
            String::from("https://test.registry.io/v2/test/some-operator/blobs/")
        );
    }

    #[test]
    fn get_blobs_url_pass() {
        let pkg = Package {
            name: String::from("some-operator"),
            channels: None,
            min_version: None,
            max_version: None,
            min_bundle: None,
        };
        let pkgs = vec![pkg];
        let ir = ImageReference {
            registry: String::from("test.registry.io"),
            namespace: String::from("test"),
            name: String::from("some-operator"),
            version: String::from("v1.0.0"),
            packages: pkgs,
        };
        let res = get_blobs_url(ir);
        assert_eq!(
            res,
            String::from("https://test.registry.io/v2/test/some-operator/blobs/")
        );
    }
}
