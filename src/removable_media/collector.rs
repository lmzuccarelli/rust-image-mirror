use crate::archive::create::MirrorStats;
use crate::error::handler::MirrorError;
use crate::image::utils::fs_handler;
use crate::image::utils::keepalive;
use custom_logger::*;
use hex::encode;
use mirror_copy::get_destination_registry;
use mirror_copy::parse_json_manifest_operator;
use mirror_copy::Manifest;
use reqwest::{Client, StatusCode};
use sha2::{Digest, Sha256};
use sha256::*;
use std::fs::{self};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::process;
use std::thread::{sleep, spawn};
use std::time::Duration;
use tar::Archive;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub async fn removable_media_disk_to_mirror(
    log: &Logging,
    from: String,
    destination: String,
    skip_blobs: bool,
    skip_verify: bool,
) -> Result<(), MirrorError> {
    // open the blobs tar
    log.hi("removable media collector mode: disk-to-mirror");

    // read stats data
    let ms_data = fs::read_to_string(format!("{}/{}", from.clone(), "mirror-stats.json"));
    let ms: MirrorStats = serde_json::from_str(ms_data.as_ref().unwrap()).unwrap();

    // first check if we have manifests (using http HEAD)
    // open the manifests tar file
    let data = std::fs::File::open(from.clone() + &"/mirror-manifests.tar");
    let mut vec_blobs: Vec<String> = Vec::new();
    let mut vec_manifests: Vec<String> = Vec::new();

    if data.is_ok() {
        log.ex(&format!("checking {} remote manifests", ms.manifest_count));
        let mut archive = Archive::new(data.unwrap());
        for (_i, file) in archive.entries().unwrap().enumerate() {
            let f = file.as_ref().unwrap().path().unwrap();
            let name = f.file_name();
            if name.is_some() {
                let op_path = f.as_ref().to_string_lossy().to_string();
                if op_path.contains(".json") {
                    let mnfst = &mut "".to_string();
                    let mut ex = file.unwrap();
                    let res = ex.read_to_string(mnfst);
                    if res.is_err() {
                        log.error(&format!("{:#?}", res.err().unwrap()));
                        process::exit(1);
                    }
                    let manifest = parse_json_manifest_operator(mnfst.to_string());
                    let path = op_path.split("/digest/").nth(0).unwrap();
                    let sha = op_path.split("/digest/").nth(1).unwrap();
                    let mut sha_clean = sha.split("-").nth(0).unwrap().to_string();
                    if !sha.contains("sha256:") {
                        sha_clean = sha.split(".json").nth(0).unwrap().to_string();
                    } else {
                        sha_clean = sha_clean.to_string().replace(":", "-").to_string();
                    }
                    let splitter = match op_path.clone() {
                        x if x.contains("operator") => "operator/".to_string(),
                        x if x.contains("release") => "release/".to_string(),
                        x if x.contains("additional") => "additional/".to_string(),
                        _ => "none".to_string(),
                    };
                    let ns = path.split(&splitter).nth(1).unwrap();

                    if res.is_ok() {
                        let req_res = check_manifest(
                            log,
                            destination.clone(),
                            ns.to_string(),
                            sha_clean.to_string(),
                            "".to_string(),
                        )
                        .await;
                        if req_res.is_err() {
                            // build the missing blobs
                            let m = manifest.as_ref().unwrap();
                            for layer in m.clone().layers.unwrap().iter() {
                                let blob =
                                    layer.digest.split("sha256:").nth(1).unwrap().to_string();
                                if !vec_blobs.contains(&blob) {
                                    vec_blobs.insert(0, blob.clone());
                                }
                            }
                            // add the config
                            let cfg_blob = m
                                .clone()
                                .config
                                .unwrap()
                                .digest
                                .split("sha256:")
                                .nth(1)
                                .unwrap()
                                .to_string();
                            if !vec_blobs.contains(&cfg_blob) {
                                vec_blobs.insert(0, cfg_blob.clone());
                            }
                            vec_manifests.insert(0, op_path.clone());
                        }
                    }
                }
            }
        }
    }

    log.debug(&format!("missing blobs {:#?}", vec_blobs));
    log.debug(&format!("missing manifests {:#?}", vec_manifests));

    if vec_blobs.len() > 0 {
        let mut blob_count = 1;
        let bar = "% completed    [--------------------------------------------------------------]"
            .to_string();
        let per_position = vec_blobs.len() as f32 / 61.0;
        fs_handler("tmp-store".to_string(), "create_dir", None)?;

        if !skip_blobs {
            log.hi(&format!("uploading {} blobs", vec_blobs.len()));
            let tars = fs::read_dir(from.clone());
            if tars.is_ok() {
                log.mid(&bar);
                for file in tars.unwrap().into_iter() {
                    let tar_file = file.as_ref().unwrap().path();
                    if tar_file.is_file() {
                        let blobs_tar = tar_file.to_string_lossy().to_string();
                        if blobs_tar.contains("mirror-blobs") {
                            let data = std::fs::File::open(blobs_tar);
                            if data.is_ok() {
                                let mut archive = Archive::new(data.unwrap());
                                for (_i, file) in archive.entries().unwrap().enumerate() {
                                    let mut x = file.unwrap();
                                    let f = x.path().unwrap();
                                    let op_path = f.as_ref().to_string_lossy().to_string();
                                    if op_path.clone().contains("/blob/") {
                                        let path = op_path.split("/blob/").nth(0).unwrap();
                                        let digest = op_path.split("/blob/").nth(1).unwrap();
                                        if vec_blobs.contains(&digest.to_string()) {
                                            let res = x.unpack("tmp-store/".to_string() + digest);
                                            if res.is_err() {
                                                log.error(&format!("{:?}", res.err().unwrap()));
                                                continue;
                                            }
                                            log.ex(&format!("  pushing blob {}", digest));
                                            // start our spinner
                                            let (keepalive_send, keepalive_recv) =
                                                keepalive::channel();
                                            let join_handle = spawn(move || {
                                                let counter = 0;
                                                let spinner = vec![
                                                    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇",
                                                    "⠏",
                                                ];
                                                while keepalive_recv.is_alive() {
                                                    for x in 0..9 {
                                                        println!("\x1b[1A \x1b[38C{}", spinner[x]);
                                                        sleep(Duration::from_millis(50));
                                                    }
                                                }
                                                counter
                                            });

                                            let req_res = process_blob(
                                                log,
                                                "tmp-store".to_string(),
                                                skip_verify,
                                                digest.to_string(),
                                                destination.clone(),
                                                path.to_string(),
                                                "".to_string(),
                                            )
                                            .await;
                                            if req_res.is_err() {
                                                println!(
                                                    "\x1b[1A \x1b[38C{}",
                                                    "\x1b[1;91m✗\x1b[0m"
                                                );
                                                log.error(&format!(
                                                    "{}",
                                                    req_res
                                                        .err()
                                                        .unwrap()
                                                        .to_string()
                                                        .to_lowercase()
                                                ));
                                                process::exit(1);
                                            }
                                            drop(keepalive_send);
                                            let _ = join_handle.join().unwrap();
                                            println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
                                            fs_handler(
                                                "tmp-store/".to_string() + digest,
                                                "remove_file",
                                                None,
                                            )?;
                                            blob_count += 1;
                                            if blob_count % 10 == 0 {
                                                let update = blob_count as f32 / per_position;
                                                let new_bar =
                                                    bar.replacen("-", "#", update.floor() as usize);
                                                log.mid(&new_bar);
                                            }
                                        }
                                    }
                                }
                            } else {
                                log.error(&format!(
                                    "reading mirroror-blobs.tar {:}",
                                    data.err().unwrap().to_string().to_lowercase()
                                ));
                                process::exit(1);
                            }
                        }
                    }
                }
                let new_bar = bar.replacen("-", "#", 62);
                log.mid(&new_bar);
            } else {
                log.error(&format!(
                    "reading mirror-blobs tar files {}",
                    tars.err().unwrap().to_string().to_lowercase()
                ));
                process::exit(1);
            }
        }
        fs_handler("tmp-store/".to_string(), "remove_dir", None)?;

        // open the metadata tar file
        let mut manifest_count = 1;
        let data = std::fs::File::open(from.clone() + &"/mirror-manifests.tar");
        let bar = "% completed    [--------------------------------------------------------------]"
            .to_string();
        let per_position = vec_manifests.len() as f32 / 61.0;

        if data.is_ok() {
            log.hi(&format!("uploading {} manifests", vec_manifests.len()));
            let mut archive = Archive::new(data.unwrap());
            log.mid(&bar);
            for (_i, file) in archive.entries().unwrap().enumerate() {
                let f = file.as_ref().unwrap().path().unwrap();
                let name = f.file_name();
                let op_path = f.as_ref().to_string_lossy().to_string();
                if name.is_some() {
                    if op_path.contains(".json") {
                        if vec_manifests.contains(&op_path) {
                            let manifest = &mut "".to_string();
                            let mut ex = file.unwrap();
                            let res = ex.read_to_string(manifest);
                            if res.is_err() {
                                log.error(&format!("{:#?}", res.err().unwrap()));
                                process::exit(1);
                            }
                            let mfst = parse_json_manifest_operator(manifest.to_string());
                            let path = op_path.split("/digest/").nth(0).unwrap();
                            let sha = op_path.split("/digest/").nth(1).unwrap();
                            let mut sha_clean = sha.split("-").nth(0).unwrap();
                            if !sha.contains("sha256:") {
                                sha_clean = sha.split(".json").nth(0).unwrap();
                            }
                            let splitter = match op_path.clone() {
                                x if x.contains("operator") => "operator/".to_string(),
                                x if x.contains("release") => "release/".to_string(),
                                x if x.contains("additional") => "additional/".to_string(),
                                _ => "none".to_string(),
                            };
                            let ns = path.split(&splitter).nth(1).unwrap();
                            log.ex(&format!("  pushing manifest {}", ns));
                            if res.is_ok() {
                                let req_res = process_manifests(
                                    log,
                                    mfst.unwrap(),
                                    destination.clone(),
                                    ns.to_string(),
                                    sha_clean.to_string(),
                                    "".to_string(),
                                )
                                .await;
                                if req_res.is_err() {
                                    println!("\x1b[1A \x1b[38C{}", "\x1b[1;91m✗\x1b[0m");
                                    log.error(&format!(
                                        "{:?}",
                                        req_res.err().unwrap().to_string().to_lowercase()
                                    ));
                                    process::exit(1);
                                }
                                println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
                                manifest_count += 1;
                                if manifest_count % 10 == 0 {
                                    let update = manifest_count as f32 / per_position;
                                    let new_bar = bar.replacen("-", "#", update.floor() as usize);
                                    log.mid(&new_bar);
                                }
                            } else {
                                println!("\x1b[1A \x1b[38C{}", "\x1b[1;91m✗\x1b[0m");
                                log.error(&format!(
                                    "{:?}",
                                    res.err().unwrap().to_string().to_lowercase()
                                ));
                                process::exit(1);
                            }
                        }
                    }
                }
            }
            let new_bar = bar.replacen("-", "#", 62);
            log.mid(&new_bar);
        } else {
            log.error(&format!(
                "reading mirror-manifest.tar {:?}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            process::exit(1);
        }
    }
    Ok(())
}

// verify_file - function to check size and sha256 hash of contents
async fn verify_file(log: &Logging, dir: String, blob_sum: String, blob_size: u64, data: Vec<u8>) {
    let f = &format!("{}/{}", dir, blob_sum);
    let res = fs::metadata(&f);
    match res {
        Ok(res) => {
            log.ex(&format!("verifying blob  {}", &blob_sum));
            assert_eq!(res.size(), blob_size);
            let hash = digest(&data);
            assert_eq!(hash, blob_sum);
        }
        Err(err) => {
            log.error(&format!("blob error  {:#?}", err.to_string()));
        }
    }
}

pub async fn process_blob(
    log: &Logging,
    dir: String,
    skip_verify: bool,
    blob: String,
    url: String,
    namespace: String,
    token: String,
) -> Result<String, MirrorError> {
    let client = Client::new();
    let client = client.clone();
    let mut header_bearer: String = "Bearer ".to_owned();
    header_bearer.push_str(&token);

    let head_url = get_destination_registry(
        url.clone(),
        namespace.clone(),
        String::from("http_blobs_digest"),
    );

    let post_url = get_destination_registry(
        url.clone(),
        namespace.clone(),
        String::from("http_blobs_uploads"),
    );

    let res = client
        .post(post_url.clone())
        .header("Accept", "application/json")
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
            "{:?}",
            res.err().unwrap().to_string().to_lowercase()
        ));
        return Err(err);
    }

    let response = res.unwrap();
    log.debug(&format!("headers {:#?}", response.headers()));
    let location = response.headers().get("Location").unwrap();

    //log.hi(&format!("pushing blob {}", &blob));

    let res_head = client
        .head(head_url.clone() + &blob)
        .header("Accept", "application/json")
        .send()
        .await;

    let head_response = res_head.unwrap();

    // if blob is not found we need to upload it
    if head_response.status() == StatusCode::NOT_FOUND {
        let mut file = File::open(dir.clone() + &"/" + &blob).await.unwrap();
        let mut vec_bytes = Vec::new();
        let _buf = file.read_to_end(&mut vec_bytes).await.unwrap();
        if !skip_verify {
            verify_file(
                log,
                dir.clone(),
                blob.clone(),
                vec_bytes.len() as u64,
                vec_bytes.clone(),
            )
            .await;
        }
        let url = location.to_str().unwrap().to_string() + &"&digest=sha256:" + &blob;
        log.debug(&format!("url  {:#?}", url.clone()));

        log.debug(&format!(
            "content info  {:#?} {:#?}",
            vec_bytes.clone().len(),
            &blob
        ));

        let res_put = client
            .put(url)
            .body(vec_bytes.clone())
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", vec_bytes.len())
            .send()
            .await;

        let res_final = res_put.unwrap();

        log.debug(&format!("result from put blob {:#?}", res_final.status()));

        if res_final.status() > StatusCode::CREATED {
            let err = MirrorError::new(&format!(
                "put blob failed with code {} : message {:#?}",
                res_final.status(),
                res_final.text().await.unwrap().to_string()
            ));
            return Err(err);
        }
    }
    Ok(String::from("ok"))
}

pub async fn process_manifests(
    log: &Logging,
    manifest: Manifest,
    url: String,
    namespace: String,
    tag_digest: String,
    token: String,
) -> Result<String, MirrorError> {
    let client = Client::new();
    let client = client.clone();
    let mut header_bearer: String = "Bearer ".to_owned();
    header_bearer.push_str(&token);

    // finally push the manifest
    let serialized_manifest = serde_json::to_string(&manifest.clone()).unwrap();
    log.debug(&format!("manifest json {:#?}", serialized_manifest.clone()));
    let put_url = get_destination_registry(
        url.clone(),
        namespace.clone(),
        String::from("http_manifest"),
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
        .header(
            "Content-Type",
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .header("Content-Length", serialized_manifest.len())
        .send()
        .await;

    let result = res_put.unwrap();
    log.trace(&format!(
        "result for manifest {:#?} {} {}",
        result.status(),
        namespace,
        put_url.clone() + &str_digest
    ));

    if result.status() != StatusCode::CREATED && result.status() != StatusCode::OK {
        let err = MirrorError::new(&format!(
            "upload manifest failed with status {:#?} : {:#?}",
            result.status(),
            result.text().await.unwrap().to_string()
        ));
        Err(err)
    } else {
        Ok(String::from("ok"))
    }
}

pub async fn check_manifest(
    log: &Logging,
    url: String,
    namespace: String,
    tag_digest: String,
    token: String,
) -> Result<String, MirrorError> {
    let client = Client::new();
    let client = client.clone();
    let header_bearer = format!("Bearer {}", token);

    let head_url = get_destination_registry(
        url.clone(),
        namespace.clone(),
        String::from("http_manifest"),
    );

    let res_put = client
        .head(head_url.clone() + &tag_digest.clone())
        .header(
            "Content-Type",
            "application/vnd.docker.distribution.manifest.v2+json",
        )
        .header("Accept", "application/json")
        .header("Authorization", header_bearer)
        .send()
        .await;

    let result = res_put.unwrap();
    log.trace(&format!(
        "result for manifest {:#?} {} {}",
        result.status(),
        namespace,
        head_url.clone() + &tag_digest
    ));

    if result.status() != StatusCode::OK {
        let err = MirrorError::new(&format!(
            "upload manifest failed with status {}",
            result.status(),
        ));
        Err(err)
    } else {
        Ok(String::from("ok"))
    }
}
