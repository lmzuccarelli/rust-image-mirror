// module copy

use async_trait::async_trait;
use futures::{stream, StreamExt};
use hex::encode;
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

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
            if !seen.contains(&truncated_image) && !exist {
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
    // push each image (blobs and manifest) referred to by the Manifest
    async fn push_image(
        &self,
        log: &Logging,
        sub_component: String,
        url: String,
        token: String,
        manifest: Manifest,
    ) -> String {
        let client = Client::new();
        let client = client.clone();

        // we iterate through all the layers
        for blob in manifest.clone().layers.unwrap().iter() {
            let _process_res = process_blob(
                log,
                &blob,
                url.clone(),
                sub_component.clone(),
                token.clone(),
            );
        }

        // mirror the config blob
        let blob = manifest.clone().config.unwrap();
        let _process_res = process_blob(
            log,
            &blob,
            url.clone(),
            sub_component.clone(),
            token.clone(),
        );

        // finally push the manifest
        let serialized_manifest = serde_json::to_string(&manifest.clone()).unwrap();
        log.trace(&format!("manifest json {:#?}", serialized_manifest.clone()));
        let put_url = get_destination_registry(
            url.clone(),
            sub_component.clone(),
            String::from("http_manifest"),
        );

        let mut hasher = Sha256::new();
        hasher.update(serialized_manifest.clone());
        let hash_bytes = hasher.finalize();
        let str_digest = encode(hash_bytes);

        let res_put = client
            .put(put_url.clone() + &str_digest.clone()[0..7])
            .body(serialized_manifest.clone())
            .header(
                "Content-Type",
                "application/vnd.docker.distribution.manifest.v2+json",
            )
            .header("Content-Length", serialized_manifest.len())
            .send()
            .await
            .unwrap();

        log.info(&format!(
            "result for manifest {} {}",
            res_put.status(),
            sub_component
        ));

        String::from("ok")
    }
}

// Refer to https://distribution.github.io/distribution/spec/api/
// for the full flow on image (container) push
//
// 1. First step is to post a blob
//    POST /v2/<name>/blobs/uploads/
//    If the POST request is successful, a 202 Accepted response will be returned
//    with Location and UUID
// 2. Check if the blob exists
//    HEAD /v2/<name>/blobs/<digest>
//    If the layer with the digest specified in digest is available, a 200 OK response will be received,
//    with no actual body content (this is according to http specification).
// 3. If it does not exist do a put
//    PUT /v2/<name>/blobs/uploads/<uuid>?digest=<digest>
//    continue for each blob in the specifid container
// 4. Finally upload the manifest
//    PUT /v2/<name>/manifests/<reference>
pub async fn process_blob(
    log: &Logging,
    blob: &Layer,
    url: String,
    sub_component: String,
    token: String,
) -> String {
    let client = Client::new();
    let client = client.clone();
    let mut header_bearer: String = "Bearer ".to_owned();
    header_bearer.push_str(&token);

    let post_url = get_destination_registry(
        url.clone(),
        sub_component.clone(),
        String::from("http_blobs_uploads"),
    );

    let res = client
        .post(post_url.clone())
        .header("Accept", "*/*")
        .send()
        .await;

    let response = res.unwrap();

    if response.status() != StatusCode::ACCEPTED {
        return String::from("ko");
    }

    log.debug(&format!("headers {:#?}", response.headers()));
    let location = response.headers().get("Location").unwrap();
    let _uuid = response.headers().get("docker-upload-uuid").unwrap();

    let head_url = get_destination_registry(
        url.clone(),
        sub_component.clone(),
        String::from("http_blobs_digest"),
    );

    let digest_no_sha = blob.digest.split(":").nth(1).unwrap().to_string();
    let path =
        String::from("./working-dir/blobs-store/") + &digest_no_sha[0..2] + &"/" + &digest_no_sha;

    let res_head = client
        .head(head_url.clone() + &blob.digest)
        .header("Accept", "*/*")
        .send()
        .await;

    let response = res_head.unwrap();

    // if blob is not found we need to upload it
    if response.status() == StatusCode::NOT_FOUND {
        let mut file = File::open(path.clone()).await.unwrap();
        let mut vec = Vec::new();
        let _buf = file.read_to_end(&mut vec).await.unwrap();
        let url = location.to_str().unwrap().to_string() + &"&digest=" + &blob.digest;
        log.trace(&format!(
            "content length  {:#?} {:#?}",
            vec.clone().len(),
            &blob.digest
        ));
        let res_put = client
            .put(url)
            .body(vec.clone())
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", vec.len())
            .send()
            .await
            .unwrap();

        log.info(&format!("result from put blob {:#?}", res_put));

        if response.status() != StatusCode::OK || response.status() != StatusCode::ACCEPTED {
            return String::from("ko");
        }
    }
    String::from("ok")
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

// get the formatted destination registry (from command line)
pub fn get_destination_registry(url: String, component: String, mode: String) -> String {
    let mut hld = url.split("docker://");
    let reg_str = hld.nth(1).unwrap();
    let mut name_str = reg_str.split("/");
    let mut reg = DestinationRegistry {
        protocol: String::from("http://"),
        registry: name_str.nth(0).unwrap().to_string(),
        name: name_str.nth(0).unwrap().to_string(),
    };

    match mode.as_str() {
        "https_blobs_uploads" => {
            reg.protocol = String::from("https://");
            return reg.protocol
                + &reg.registry
                + &"/v2/"
                + &reg.name
                + &"/"
                + &component
                + &"/blobs/uploads/";
        }
        "http_blobs_uploads" => {
            return reg.protocol
                + &reg.registry
                + &"/v2/"
                + &reg.name
                + &"/"
                + &component
                + &"/blobs/uploads/"
        }
        "http_blobs_digest" => {
            return reg.protocol
                + &reg.registry
                + &"/v2/"
                + &reg.name
                + &"/"
                + &component
                + &"/blobs/"
        }
        "http_manifest" => {
            return reg.protocol
                + &reg.registry
                + &"/v2/"
                + &reg.name
                + &"/"
                + &component
                + &"/manifests/"
        }
        _ => {
            return reg.protocol
                + &reg.registry
                + &"/v2/"
                + &reg.name
                + "/"
                + &component
                + &"/blobs/uploads/"
        }
    };
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
            packages: Some(pkgs),
        };
        let res = get_blobs_url(ir);
        assert_eq!(
            res,
            String::from("https://test.registry.io/v2/test/some-operator/blobs/")
        );
    }
}
