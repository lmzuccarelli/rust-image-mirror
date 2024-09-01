use crate::mirror::utils::verify_file;
use async_trait::async_trait;
use custom_logger::*;
use hex::encode;
use mirror_copy::Manifest;
use mirror_error::MirrorError;
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
pub struct ImplProcessImageInterface {}

#[async_trait]
pub trait ProcessImageInterface {
    async fn process_manifests(
        &self,
        log: &Logging,
        url: String,
        namespace: String,
        manifest: Manifest,
        tag_digest: String,
        token: String,
    ) -> Result<String, MirrorError>;

    async fn check_manifest(
        &self,
        log: &Logging,
        url: String,
        namespace: String,
        tag_digest: String,
        token: String,
    ) -> Result<String, MirrorError>;

    async fn process_blob(
        &self,
        log: &Logging,
        url: String,
        namespace: String,
        dir: String,
        skip_verify: bool,
        blob: String,
        token: String,
    ) -> Result<String, MirrorError>;
}

#[async_trait]
impl ProcessImageInterface for ImplProcessImageInterface {
    async fn process_manifests(
        &self,
        _log: &Logging,
        url: String,
        namespace: String,
        manifest: Manifest,
        tag_digest: String,
        token: String,
    ) -> Result<String, MirrorError> {
        let client = Client::new();
        let client = client.clone();
        let mut header_bearer: String = "Bearer ".to_owned();
        header_bearer.push_str(&token);

        // finally push the manifest
        let serialized_manifest = serde_json::to_string(&manifest.clone()).unwrap();

        let put_url = format!(
            "https://{}/v2/{}/manifests/",
            url.clone(),
            namespace.clone(),
        );

        let str_digest: String;
        if tag_digest == "".to_string() {
            let mut hasher = Sha256::new();
            hasher.update(serialized_manifest.clone());
            let hash_bytes = hasher.finalize();
            str_digest = encode(hash_bytes);
        } else {
            str_digest = tag_digest.replace(":", "-");
        }
        let res_put = client
            .put(put_url.clone() + &str_digest.clone())
            .body(serialized_manifest.clone())
            .header("Authorization", header_bearer)
            .header(
                "Content-Type",
                "application/vnd.docker.distribution.manifest.v2+json",
            )
            .header("Content-Length", serialized_manifest.len())
            .send()
            .await;

        let result = res_put.unwrap();
        if result.status() != StatusCode::CREATED && result.status() != StatusCode::OK {
            let err = MirrorError::new(&format!(
                "[process_manifests] upload manifest failed with status {} : {}",
                result.status(),
                result.text().await.unwrap().to_string()
            ));
            Err(err)
        } else {
            Ok(String::from("ok"))
        }
    }
    async fn check_manifest(
        &self,
        _log: &Logging,
        url: String,
        namespace: String,
        tag_digest: String,
        token: String,
    ) -> Result<String, MirrorError> {
        let client = Client::new();
        let client = client.clone();
        let header_bearer = format!("Bearer {}", token);

        let head_url = format!(
            "https://{}/v2/{}/manifests/{}",
            url.clone(),
            namespace.clone(),
            tag_digest,
        );

        let res_head = client
            .head(head_url.clone())
            .header("Accept", "application/json")
            .header("Authorization", header_bearer)
            .send()
            .await;

        if res_head.is_ok() {
            let result = res_head.unwrap();
            if result.status() != StatusCode::OK {
                let err = MirrorError::new(&format!(
                    "upload manifest failed with status {}",
                    result.status(),
                ));
                Err(err)
            } else {
                Ok(String::from("ok"))
            }
        } else {
            let err = MirrorError::new(&format!(
                "[check_manifest] upload manifest failed {}",
                res_head.err().unwrap().to_string().to_lowercase(),
            ));
            Err(err)
        }
    }
    async fn process_blob(
        &self,
        log: &Logging,
        url: String,
        namespace: String,
        dir: String,
        verify_blobs: bool,
        blob: String,
        token: String,
    ) -> Result<String, MirrorError> {
        let client = Client::new();
        let client = client.clone();
        let mut header_bearer: String = "Bearer ".to_owned();
        header_bearer.push_str(&token);

        let head_url = format!(
            "https://{}/v2/{}/blobs/sha256:{}",
            url.clone(),
            namespace.clone(),
            blob.clone()
        );

        let res_head = client
            .head(head_url.clone())
            .header("Authorization", header_bearer.clone())
            .send()
            .await;

        if res_head.unwrap().status() == StatusCode::NOT_FOUND {
            let post_url = format!(
                "https://{}/v2/{}/blobs/uploads/",
                url.clone(),
                namespace.clone(),
            );

            let res = client
                .post(post_url.clone())
                .header("Authorization", header_bearer.clone())
                .send()
                .await;

            if res.is_ok() {
                if res.as_ref().unwrap().status() != StatusCode::ACCEPTED {
                    let err = MirrorError::new(&format!(
                        "initial post failed with status {:#?}",
                        res.unwrap().status()
                    ));
                    return Err(err);
                }
            } else {
                let err = MirrorError::new(&format!(
                    "{}",
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }

            let response = res.unwrap();
            let location = response.headers().get("Location").unwrap();

            let res_patch = client
                .patch(location.to_str().unwrap())
                .header("Authorization", header_bearer.clone())
                .header("Accept", "application/json")
                .send()
                .await;

            let res_response = res_patch.unwrap();

            if res_response.status() == StatusCode::ACCEPTED {
                let mut file = File::open(dir.clone() + &"/" + &blob).await.unwrap();
                let mut vec_bytes = Vec::new();
                let _buf = file.read_to_end(&mut vec_bytes).await.unwrap();
                if verify_blobs {
                    let res = verify_file(
                        log,
                        dir.clone(),
                        blob.clone(),
                        vec_bytes.len() as u64,
                        vec_bytes.clone(),
                    )
                    .await;
                    if res.is_err() {
                        let err = MirrorError::new(&format!("{}", res.err().unwrap().to_string(),));
                        return Err(err);
                    }
                }
                let url = location.to_str().unwrap().to_string() + &"?digest=sha256:" + &blob;

                let res_put = client
                    .put(url)
                    .body(vec_bytes.clone())
                    .header("Authorization", header_bearer.clone())
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Length", vec_bytes.len())
                    .send()
                    .await;

                let res_final = res_put.unwrap();

                if res_final.status() > StatusCode::CREATED {
                    let err = MirrorError::new(&format!(
                        "[process_blob] put blob failed with code {} : message {:#?}",
                        res_final.status(),
                        res_final.text().await.unwrap().to_string()
                    ));
                    return Err(err);
                }
            }
        }
        Ok(String::from("ok"))
    }
}
