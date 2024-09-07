use crate::archive::create::MirrorStats;
use crate::MirrorParameters;
use custom_logger::*;
use mirror_auth::{get_token, ImplTokenInterface};
use mirror_copy::UploadImageInterface;
use mirror_error::MirrorError;
use mirror_utils::{fs_handler, keepalive, parse_json_manifest_operator};
use std::fs::{self};
use std::io::Read;
use std::thread::{sleep, spawn};
use std::time::Duration;
use tar::Archive;

pub async fn removable_media_disk_to_mirror<T: UploadImageInterface>(
    g_impl: T,
    log: &Logging,
    from: String,
    destination: String,
    mp: MirrorParameters,
) -> Result<(), MirrorError> {
    // open the blobs tar
    log.hi("[removable_media_disk_to_mirror] collector mode: disk-to-mirror");

    // read stats data
    let ms_data = fs_handler(
        format!("{}/{}", from.clone(), "mirror-stats.json"),
        "read",
        None,
    )
    .await?;
    let ms: MirrorStats = serde_json::from_str(&ms_data.clone()).unwrap();

    // first check if we have manifests (using http HEAD)
    // open the manifests tar file
    let data = std::fs::File::open(from.clone() + &"/mirror-manifests.tar");
    let mut vec_blobs: Vec<String> = Vec::new();
    let mut vec_manifests: Vec<String> = Vec::new();
    let t_impl = ImplTokenInterface {};

    let url = destination.split("docker://").nth(1).unwrap();
    let registry = url.split("/").nth(0).unwrap();
    let registry_namespace = url.split("/").nth(1).unwrap();

    if data.is_ok() {
        log.ex(&format!(
            "[removable_media_disk_to_mirror] checking {} remote manifests",
            ms.manifest_count
        ));
        let bar = "% completed    [---------------------------------------------------------------------]"
            .to_string();
        let per_position = ms.manifest_count as f32 / 74.0;
        let mut count = 1;
        let mut archive = Archive::new(data.unwrap());
        log.mid(&bar);
        for (_i, file) in archive.entries().unwrap().enumerate() {
            let f = file.as_ref().unwrap().path().unwrap();
            let name = f.file_name();
            if name.is_some() {
                let op_path = f.as_ref().to_string_lossy().to_string();
                if op_path.contains(".json") {
                    let mnfst = &mut "".to_string();
                    let mut ex = file.unwrap();
                    let res = ex.read_to_string(mnfst);
                    log.debug(&format!("{:#?}", res.as_ref().unwrap()));
                    let manifest = parse_json_manifest_operator(mnfst.to_string())?;
                    let (path, sha) = op_path.split_once("/digest/").unwrap();
                    let mut sha_clean = sha.split("-").nth(0).unwrap().to_string();
                    if !sha.contains("sha256:") {
                        sha_clean = sha.split(".json").nth(0).unwrap().to_string();
                    } else {
                        sha_clean = sha_clean.to_string().replace(":", "-").to_string();
                    }
                    let splitter = match op_path.clone() {
                        x if x.contains("operator") => "operator/".to_string(),
                        x if x.contains("ocp-release") => "ocp-release/".to_string(),
                        x if x.contains("additional") => "additional/".to_string(),
                        _ => "none".to_string(),
                    };
                    let ns = path.split(&splitter).nth(1);
                    if ns.is_none() {
                        continue;
                    }
                    if res.is_ok() {
                        log.ex(&format!(
                            "  checking manifest {} {} ",
                            ns.unwrap(),
                            sha_clean.clone()
                        ));
                        // start our spinner
                        let (keepalive_send, keepalive_recv) = keepalive::channel();
                        let join_handle = spawn(move || {
                            let counter = 0;
                            let spinner = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            while keepalive_recv.is_alive() {
                                for x in 0..9 {
                                    println!("\x1b[1A \x1b[38C{}", spinner[x]);
                                    sleep(Duration::from_millis(50));
                                }
                            }
                            counter
                        });
                        let local_token = get_token(
                            t_impl.clone(),
                            log,
                            registry.to_string(),
                            format!("{}/{}", registry_namespace, ns.unwrap()),
                            mp.tls_verify,
                        )
                        .await?;
                        let req_res = g_impl
                            .check_manifest(
                                log,
                                registry.to_string(),
                                format!("{}/{}", registry_namespace, ns.unwrap()),
                                sha_clean.to_string(),
                                local_token.clone(),
                            )
                            .await;
                        drop(keepalive_send);
                        let _ = join_handle.join().unwrap();

                        if req_res.is_err() {
                            // build the missing blobs
                            let m = manifest.clone();
                            for layer in m.clone().layers.unwrap().iter() {
                                let blob =
                                    layer.digest.split("sha256:").nth(1).unwrap().to_string();
                                vec_blobs.insert(0, blob.clone());
                            }
                            // add the config
                            let cfg_blob = m
                                .clone()
                                .config
                                .unwrap()
                                .digest
                                .clone()
                                .split("sha256:")
                                .nth(1)
                                .unwrap()
                                .to_string();
                            vec_blobs.insert(0, cfg_blob.clone());
                            vec_manifests.insert(0, op_path.clone());
                            println!("\x1b[1A \x1b[38C{}", "\x1b[1;93m*\x1b[0m");
                        } else {
                            println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
                        }
                        if count % 10 == 0 {
                            let update = count as f32 / per_position;
                            let new_bar = bar.replacen("-", "#", update.floor() as usize);
                            log.mid(&new_bar);
                        }
                        count += 1;
                    }
                }
            }
        }
    } else {
        return Err(MirrorError::new(&format!(
            "[removable_media_disk_to_mirror] reading mirror-manifest.tar {}",
            data.err().unwrap().to_string().to_lowercase()
        )));
    }

    log.debug(&format!(
        "[removable_media_disk_to_mirror] missing blobs {:#?}",
        vec_blobs
    ));
    log.debug(&format!(
        "[removable_media_disk_to_mirror] missing manifests {:#?}",
        vec_manifests
    ));

    if vec_blobs.len() > 0 {
        let mut blob_count = 1;
        let bar = "% completed    [---------------------------------------------------------------------]"
            .to_string();
        let per_position = vec_blobs.len() as f32 / 74.0;
        fs_handler("tmp-store".to_string(), "create_dir", None).await?;

        if !mp.skip_blob_upload {
            log.hi(&format!("[removable_media_disk_to_mirror] uploading blobs"));
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
                                        let (path, digest) = op_path.split_once("/blob/").unwrap();
                                        if vec_blobs.contains(&digest.to_string()) {
                                            let res = x.unpack("tmp-store/".to_string() + digest);
                                            log.debug(&format!("[removable_media_disk_to_mirror] result from unpack {:?}",res.unwrap()));
                                            log.ex(&format!("  pushing blob sha256:{}", digest));
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

                                            // cleanup path
                                            let updated_path = path.replace("./", "");
                                            let local_token = get_token(
                                                t_impl.clone(),
                                                log,
                                                registry.to_string(),
                                                format!(
                                                    "{}/{}",
                                                    registry_namespace,
                                                    updated_path.clone(),
                                                ),
                                                mp.tls_verify,
                                            )
                                            .await?;
                                            let req_res = g_impl
                                                .process_blob(
                                                    log,
                                                    registry.to_string(),
                                                    format!(
                                                        "{}/{}",
                                                        registry_namespace,
                                                        updated_path.to_string()
                                                    ),
                                                    "tmp-store".to_string(),
                                                    mp.verify_blobs,
                                                    digest.to_string(),
                                                    local_token.clone(),
                                                )
                                                .await;
                                            drop(keepalive_send);
                                            let _ = join_handle.join().unwrap();

                                            if req_res.is_err() {
                                                println!(
                                                    "\x1b[1A \x1b[38C{}",
                                                    "\x1b[1;91m✗\x1b[0m"
                                                );
                                                //process::exit(1);
                                            } else {
                                                println!(
                                                    "\x1b[1A \x1b[38C{}",
                                                    "\x1b[1;92m✓\x1b[0m"
                                                );
                                            }
                                            fs_handler(
                                                "tmp-store/".to_string() + digest,
                                                "remove_file",
                                                None,
                                            )
                                            .await?;
                                            if blob_count % 10 == 0 {
                                                let update = blob_count as f32 / per_position;
                                                let new_bar =
                                                    bar.replacen("-", "#", update.floor() as usize);
                                                log.mid(&new_bar);
                                            }
                                            blob_count += 1;
                                        }
                                    }
                                }
                            } else {
                                return Err(MirrorError::new(&format!(
                                    "[removable_media_disk_to_mirror] reading mirror-blobs.tar {:}",
                                    data.err().unwrap().to_string().to_lowercase()
                                )));
                            }
                        }
                    }
                }
                let new_bar = bar.replacen("-", "#", 69);
                log.mid(&new_bar);
            }
        }
        fs_handler("tmp-store/".to_string(), "remove_dir", None).await?;

        // open the metadata tar file
        let mut manifest_count = 1;
        let data = std::fs::File::open(from.clone() + &"/mirror-manifests.tar");
        let bar = "% completed    [---------------------------------------------------------------------]"
            .to_string();
        let per_position = vec_manifests.len() as f32 / 74.0;

        if data.is_ok() {
            log.hi(&format!(
                "[removable_media_disk_to_mirror] uploading {} manifests",
                vec_manifests.len()
            ));
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
                            log.debug(&format!("{:?}", res.as_ref().unwrap()));
                            let mfst = parse_json_manifest_operator(manifest.to_string())?;
                            let (path, sha) = op_path.split_once("/digest/").unwrap();
                            let mut sha_clean = sha.split("-").nth(0).unwrap();
                            if !sha.contains("sha256:") {
                                sha_clean = sha.split(".json").nth(0).unwrap();
                            }
                            let splitter = match op_path.clone() {
                                x if x.contains("operator") => "operator/".to_string(),
                                x if x.contains("ocp-release") => "ocp-release/".to_string(),
                                x if x.contains("additional") => "additional/".to_string(),
                                _ => "none".to_string(),
                            };
                            let ns = path.split(&splitter).nth(1).unwrap();
                            let local_token = get_token(
                                t_impl.clone(),
                                log,
                                registry.to_string(),
                                format!("{}/{}", registry_namespace, ns.to_string()),
                                mp.tls_verify,
                            )
                            .await?;
                            log.ex(&format!("  pushing manifest {} {}", ns, sha_clean));
                            if res.is_ok() {
                                let req_res = g_impl
                                    .process_manifests(
                                        log,
                                        registry.to_string(),
                                        format!("{}/{}", registry_namespace, ns.to_string()),
                                        mfst.clone(),
                                        sha_clean.to_string(),
                                        local_token.clone(),
                                    )
                                    .await;
                                if req_res.is_err() {
                                    println!("\x1b[1A \x1b[38C{}", "\x1b[1;91m✗\x1b[0m");
                                } else {
                                    println!("\x1b[1A \x1b[38C{}", "\x1b[1;92m✓\x1b[0m");
                                }
                                manifest_count += 1;
                                if manifest_count % 10 == 0 {
                                    let update = manifest_count as f32 / per_position;
                                    let new_bar = bar.replacen("-", "#", update.floor() as usize);
                                    log.mid(&new_bar);
                                }
                            } else {
                                println!("\x1b[1A \x1b[38C{}", "\x1b[1;91m✗\x1b[0m");
                            }
                        }
                    }
                }
            }
            let new_bar = bar.replacen("-", "#", 69);
            log.mid(&new_bar);
        } else {
            return Err(MirrorError::new(&format!(
                "[removable_media_disk_to_mirror] reading mirror-manifest.tar {}",
                data.err().unwrap().to_string().to_lowercase()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use async_trait::async_trait;
    use mirror_copy::Manifest;
    use std::collections::HashMap;
    use std::path::Path;
    use std::time;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn removable_media_disk_to_mirror_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };

        #[derive(Clone)]
        struct Fake {}

        #[async_trait]
        impl UploadImageInterface for Fake {
            async fn process_manifests(
                &self,
                _log: &Logging,
                url: String,
                _namespace: String,
                _manifest: Manifest,
                _tag_digest: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                let millis = time::Duration::from_millis(100);
                sleep(millis);
                if url.contains("manifest-error") {
                    return Err(MirrorError::new("processing manifest"));
                }
                Ok("ok".to_string())
            }

            async fn check_manifest(
                &self,
                _log: &Logging,
                _url: String,
                _namespace: String,
                _tag_digest: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                let millis = time::Duration::from_millis(100);
                sleep(millis);
                Err(MirrorError::new("manifests missing"))
            }

            async fn process_blob(
                &self,
                _log: &Logging,
                url: String,
                _namespace: String,
                _dir: String,
                _skip_verify: bool,
                _blob: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                let millis = time::Duration::from_millis(100);
                sleep(millis);
                if url.contains("blob-error") {
                    return Err(MirrorError::new("processing blob"));
                }
                Ok("ok".to_string())
            }
        }

        let mp = MirrorParameters {
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

        let fake = Fake {};
        let updated_url = format!("{}/{}", "docker://localhost:5000/test", "test-namespace");
        let res = aw!(removable_media_disk_to_mirror(
            fake.clone(),
            log,
            "test-artifacts/do-not-delete/artifacts".to_string(),
            updated_url,
            mp.clone(),
        ));
        assert_eq!(res.is_ok(), true);

        let updated_url = format!(
            "{}/{}",
            "docker://localhost:5000/manifest-error", "manifest-error"
        );
        let res = aw!(removable_media_disk_to_mirror(
            fake.clone(),
            log,
            "test-artifacts/do-not-delete/artifacts".to_string(),
            updated_url,
            mp.clone(),
        ));
        assert_eq!(res.is_ok(), true);

        let updated_url = format!("{}/{}", "docker://localhost:5000/blob-error", "blob-error");
        let res = aw!(removable_media_disk_to_mirror(
            fake.clone(),
            log,
            "test-artifacts/do-not-delete/artifacts".to_string(),
            updated_url,
            mp.clone(),
        ));
        assert_eq!(res.is_ok(), true);

        if Path::new("test-artifacts/missing-files/artifacts/mirror-manifests.tar").exists() {
            fs::remove_file("test-artifacts/missing-files/artifacts/mirror-manifests.tar")
                .expect("should delete tar file");
        }
        let updated_url = format!("{}/{}", "docker://localhost:5000/test", "test-namespace");
        let res = aw!(removable_media_disk_to_mirror(
            fake.clone(),
            log,
            "test-artifacts/missing-files/artifacts".to_string(),
            updated_url,
            mp.clone(),
        ));
        assert_eq!(res.is_err(), true);
    }
}
