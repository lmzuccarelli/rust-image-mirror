use crate::api::schema::MirrorImageInfo;
use crate::batch::worker::execute_batch;
use crate::config::load::*;
use crate::mirror::utils::*;
use crate::MirrorParameters;
use custom_logger::*;
use mirror_auth::*;
use mirror_copy::{parse_json_manifestlist, FsLayer, RegistryInterface};
use mirror_error::MirrorError;
use std::collections::HashMap;

// collect all additional images
pub async fn additional_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    additional: Vec<Image>,
    mp: MirrorParameters,
) -> Result<(), MirrorError> {
    log.hi("additional images collector mode: mirror-to-disk");

    //let blobs_dir = dir.clone() + "/blobs-store/";
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();
    let mut vec_fslayers: Vec<FsLayer> = vec![];
    let mut fslayers: HashMap<String, Vec<FsLayer>> = HashMap::new();
    // parse the config
    for image in additional.iter() {
        let ir = parse_image(log, image.name.clone());
        log.debug(&format!("image refs {:#?}", ir));
        let token = get_token(log, ir.clone().registry).await?;
        // set both url and cache params
        let manifest_url = format!(
            "https://{}/v2/{}/{}/manifests/{}",
            ir.registry, ir.namespace, ir.name, ir.version
        );
        // set up dir to store all manifests
        fs_handler(
            format!("{}/{}", mp.dir.clone(), "/manifests/additional"),
            "create_dir",
            None,
        )
        .await?;
        let manifest_list: String;
        let working_dir_cache = format!("{}/manifests/additional", mp.dir.clone());
        let mflist_file = format!(
            "{}/{}-{}.json",
            working_dir_cache,
            image.name.clone().replace("/", "-"),
            "list"
        );
        if !(mp.skip_manifest_check == "additional" || mp.skip_manifest_check == "all") {
            log.mid(&format!(
                "api call manifest list for {:#}",
                ir.registry.clone() + &"/" + &ir.namespace.clone() + "/" + &ir.name.clone()
            ));
            let res = reg_con
                .get_manifest(manifest_url.clone(), token.clone())
                .await?;
            //write manifestlist to disk
            fs_handler(mflist_file.clone(), "write", Some(res.clone())).await?;
            manifest_list = res.clone();
        } else {
            log.debug(&format!("manifest list file {}", mflist_file));
            let res = fs_handler(mflist_file.clone(), "read", None).await?;
            manifest_list = res.clone();
        }
        let mem_manifest_list = manifest_list.clone();
        let ml = parse_json_manifestlist(mem_manifest_list.clone())?;
        for m in ml.clone().manifests.iter() {
            let mut ir_url = ir.clone();
            let arch = m.platform.as_ref().unwrap().architecture.to_string();
            ir_url.version = m.digest.as_ref().unwrap().to_string();

            let arch_manifest_json = format!(
                "{}/manifests/additional/{}-{}.json",
                mp.dir.clone(),
                ir_url.version.clone(),
                arch.clone(),
            );

            if !(mp.skip_manifest_check == "additional" || mp.skip_manifest_check == "all")
                && !mp.dry_run
            {
                log.mid(&format!(
                    "api call for arch manifest {:#}",
                    ir_url.registry.clone()
                        + &"/"
                        + &ir_url.namespace.clone()
                        + "/"
                        + &ir_url.name.clone()
                ));
                let mnfst_url = format!(
                    "https://{}/v2/{}/{}/manifests/{}",
                    ir_url.registry, ir_url.namespace, ir_url.name, ir_url.version
                );
                let res = reg_con
                    .get_manifest(mnfst_url.clone(), token.clone())
                    .await?;
                fs_handler(arch_manifest_json.clone(), "write", Some(res.clone())).await?;
            }
        }
        // at this stage we are confident all manifests are on disk (cache)
        // lets verify all related blobs
        let mlm = read_and_parse_oci_manifestlist(mflist_file)?;
        // contruct mirror meta data (used for diskToMirror flow)
        let mut img_ref = MirrorImageInfo {
            reference: image.clone().name,
            name: ir.name.clone(),
            arch: "all".to_string(),
            tag: None,
            namespace: ir.namespace.clone() + &"/" + &ir.name,
            digest: "".to_string(),
            manifest_type: "list".to_string(),
            created: "".to_string(),
            mirror_type: "additional".to_string(),
            bundle: None,
        };
        if ir.version.clone().contains("sha256:") {
            img_ref.digest = ir.version.clone();
        } else {
            img_ref.tag = Some(ir.version.clone());
        }
        image_ref_tracker.insert(0, img_ref.clone());
        for m in mlm.clone().manifests.iter() {
            let arch = m.platform.as_ref().unwrap().architecture.to_string();
            let arch_manifest_json = format!(
                "{}/{}-{}.json",
                working_dir_cache.clone(),
                m.digest.as_ref().unwrap().clone(),
                arch.clone(),
            );
            log.debug(&format!(
                "reading arch manifest json from {}",
                arch_manifest_json
            ));
            let arch_manifest = read_and_parse_oci_manifest(arch_manifest_json.clone())?;
            for l in arch_manifest.layers.as_ref().unwrap().iter() {
                let fsl = FsLayer {
                    blob_sum: l.digest.clone(),
                    original_ref: Some(ir.name.clone()),
                    size: Some(l.size),
                    number: None,
                };
                vec_fslayers.insert(0, fsl.clone());
            }
            let cfg = arch_manifest.config.unwrap();
            let fsl = FsLayer {
                blob_sum: cfg.digest.clone(),
                original_ref: Some(ir.name.clone()),
                size: Some(cfg.size),
                number: None,
            };
            vec_fslayers.insert(0, fsl);
            let img_ref = MirrorImageInfo {
                reference: image.clone().name,
                name: ir.name.clone(),
                arch: arch.clone(),
                tag: None,
                namespace: ir.namespace.clone() + &"/" + &ir.name,
                digest: m.digest.as_ref().unwrap().clone(),
                manifest_type: "manifest".to_string(),
                created: "".to_string(),
                mirror_type: "additional".to_string(),
                bundle: None,
            };
            image_ref_tracker.insert(0, img_ref.clone());
        }
        let url = format!(
            "https://{}/v2/{}/{}/blobs/",
            ir.registry, ir.namespace, ir.name
        );
        fslayers.insert(url.clone(), vec_fslayers.clone());
    }
    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs_handler(
        mp.dir.clone() + &"/mirror-metadata/additional-image-reference.json",
        "write",
        Some(serialized_manifest),
    )
    .await?;
    // if dry run set don't execute blob concurrency section
    if mp.dry_run {
        log.ex("dry-run flag detected");
        let mut buf = String::from("");
        let mut file = String::from("/mirror-metadata/additional-image-reference.json");
        // used to override the reference file
        let mdir_file = mp.generic_override.get("additional-image-reference");
        if mdir_file.is_some() {
            file = mdir_file.unwrap().to_string();
            log.debug(&format!(
                "using file override {}{}",
                mp.dir.clone(),
                file.clone()
            ));
        }
        let md = read_and_parse_metadata(mp.dir.clone() + &file)?;
        for mii in md.iter() {
            if mp.architectures.contains(&mii.arch.to_string()) {
                let src = &format!("{}{}", "docker://", mii.reference);
                let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                buf = buf + &format!("{}={}\n", src, dest);
            }
        }
        fs_handler(
            mp.dir.clone() + "/mappings/additional-mapping.txt",
            "write",
            Some(buf),
        )
        .await?;
        log.mid(&format!(
            "created additional images mapping file in folder {}",
            mp.dir.clone() + &"/mappings/",
        ));
    } else {
        let map = remove_duplicates(mp.dir.clone(), fslayers);
        let res = execute_batch(log, mp.dir.clone(), mp.verify_blobs, mp.tls_verify, map).await;
        if res.is_err() {
            return Err(res.err().unwrap());
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
    use std::fs;

    #[test]
    fn additional_mirror_to_disk_pass() {
        let log = &Logging {
            log_level: Level::TRACE,
        };

        macro_rules! aw {
            ($e:expr) => {
                tokio_test::block_on($e)
            };
        }

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

        #[derive(Clone)]
        struct Fake {}

        #[async_trait]
        impl RegistryInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, MirrorError> {
                let content = match url.clone() {
                    x if x.contains("test-additional-image/manifests/latest") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/manifest-list.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("test-additional-image/manifests/sha256:65e311ef7036acc3692d291403656b840fd216d120b3c37af768f91df050257d") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/sha256:65e311ef7036acc3692d291403656b840fd216d120b3c37af768f91df050257d-amd64.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("test-additional-image/manifests/sha256:443bba00c2bfa4aa08f4fb47cfa2aa80de3302420a52452ba5e790a69dcaf60e") => { 
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/sha256:443bba00c2bfa4aa08f4fb47cfa2aa80de3302420a52452ba5e790a69dcaf60e-arm64.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("test-additional-image/manifests/sha256:50a900cc4ecd2a792731e25c4d641074ebef6f5465b561d1ebf4eb9386807787") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/sha256:50a900cc4ecd2a792731e25c4d641074ebef6f5465b561d1ebf4eb9386807787-ppc64le.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("test-additional-image/manifests/sha256:c6938da14fd3c55635d89a59ba2365014651d4b2b86366d8e4ef4db1c99feb1a") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/sha256:c6938da14fd3c55635d89a59ba2365014651d4b2b86366d8e4ef4db1c99feb1a-s390x.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("bad-additional-image/manifests/latest") => {
                        let content = fs::read_to_string(
                            "test-artifacts/test-additional-image/bad-manifest-list.json",
                        )
                        .expect("should read operator-index manifest file");
                        content
                    }
                    x if x.contains("error-additional-image/manifests/latest") => {
                        let err = MirrorError::new("forced error");
                        return Err(err);
                    }
                    _ => {
                       "".to_string()
                    }
                };
                Ok(content.to_string())
            }

            async fn get_blobs(
                &self,
                _log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _layers: Vec<FsLayer>,
            ) -> Result<String, MirrorError> {
                Ok("ok".to_string())
            }

            async fn push_image(
                &self,
                _log: &Logging,
                _dir: String,
                _sub_component: String,
                _url: String,
                _token: String,
                _manifest: Manifest,
            ) -> Result<String, MirrorError> {
                Ok("ok".to_string())
            }
        }

        let fake = Fake {};

        let img = Image {
            name: format!(
                "{}/test/test-additional-image:latest",
                url.split("http://").nth(1).unwrap()
            ),
        };

        fs::create_dir_all("./test-artifacts/mirror-metadata")
            .expect("should create mirror-metadata test folder");
        fs::create_dir_all("./test-artifacts/mappings")
            .expect("should create mappings test folder");

        // skip manifest check none
        // dry-run false
        // verify-blobs false
        // tls_verify false
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

        let images = vec![img.clone()];
        let res = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp,
        ));
        assert_eq!(res.is_ok(), true);

        // dry run true
        // skip_manifest_check all
        let mp_dr = MirrorParameters {
            architectures: vec![
                "amd64".to_string(),
                "arm64".to_string(),
                "ppc64le".to_string(),
                "s390x".to_string(),
            ],
            destination: "".to_string(),
            dry_run: true,
            dir: "./test-artifacts".to_string(),
            from: "".to_string(),
            skip_blob_upload: false,
            skip_manifest_check: "all".to_string(),
            tls_verify: false,
            verify_blobs: false,
            generic_override: HashMap::new(),
            rebuild_catalogs: Some(false),
        };

        let res_dr = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp_dr,
        ));
        assert_eq!(res_dr.is_ok(), true);

        // dry run true
        // skip_manifest_check all
        // set file override
        let mut f_override: HashMap<String, String> = HashMap::new();
        f_override.insert("additional-image-reference".to_string(), "nada".to_string());
        let mp_dr = MirrorParameters {
            architectures: vec![
                "amd64".to_string(),
                "arm64".to_string(),
                "ppc64le".to_string(),
                "s390x".to_string(),
            ],
            destination: "".to_string(),
            dry_run: true,
            dir: "./test-artifacts".to_string(),
            from: "".to_string(),
            skip_blob_upload: false,
            skip_manifest_check: "all".to_string(),
            tls_verify: false,
            verify_blobs: false,
            generic_override: f_override.clone(),
            rebuild_catalogs: Some(false),
        };
        let res_k = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp_dr,
        ));
        if res_k.is_err() {
            log.error(&format!(
                "result {:#?}",
                res_k.as_ref().err().unwrap().to_string()
            ));
        }
        assert_eq!(res_k.is_err(), true);

        // dry run true
        // skip_manifest_check all
        // set file override
        let mut f_override: HashMap<String, String> = HashMap::new();
        f_override.insert(
            "additional-image-reference".to_string(),
            "/test-additional-image/bad-manifest-list.json".to_string(),
        );
        let mp_dr = MirrorParameters {
            architectures: vec![
                "amd64".to_string(),
                "arm64".to_string(),
                "ppc64le".to_string(),
                "s390x".to_string(),
            ],
            destination: "".to_string(),
            dry_run: true,
            dir: "./test-artifacts".to_string(),
            from: "".to_string(),
            skip_blob_upload: false,
            skip_manifest_check: "all".to_string(),
            tls_verify: false,
            verify_blobs: false,
            generic_override: f_override.clone(),
            rebuild_catalogs: Some(false),
        };
        let res_k = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp_dr,
        ));
        if res_k.is_err() {
            log.error(&format!(
                "result {:#?}",
                res_k.as_ref().err().unwrap().to_string()
            ));
        }
        assert_eq!(res_k.is_err(), true);

        // dry run false
        // skip_manifest_check all
        let mp_dr = MirrorParameters {
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
            skip_manifest_check: "all".to_string(),
            tls_verify: false,
            verify_blobs: false,
            generic_override: HashMap::new(),
            rebuild_catalogs: Some(false),
        };

        // remove manifests directory
        fs::remove_dir_all("./test-artifacts/manifests").expect("should delete test directory");
        let res_x = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp_dr,
        ));
        if res_x.is_err() {
            log.error(&format!(
                "result {:#?}",
                res_x.as_ref().err().unwrap().to_string()
            ));
        }
        assert_eq!(res_x.is_err(), true);

        // test error should fail
        let img = Image {
            name: format!(
                "{}/test/bad-additional-image:latest",
                url.split("http://").nth(1).unwrap()
            ),
        };

        let mp = MirrorParameters {
            architectures: vec![],
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

        // skip manifest check none
        // dry-run false
        // verify-blobs false
        // tls_verify false
        let images = vec![img.clone()];
        let res = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp,
        ));
        if res.is_err() {
            log.error(&format!(
                "result {:#?}",
                res.as_ref().err().unwrap().to_string()
            ));
        }
        assert_eq!(res.is_err(), true);

        // test error should fail
        let img = Image {
            name: format!(
                "{}/test/error-additional-image:latest",
                url.split("http://").nth(1).unwrap()
            ),
        };

        let mp = MirrorParameters {
            architectures: vec![],
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

        // skip manifest check none
        // dry-run false
        // verify-blobs false
        // tls_verify false
        let images = vec![img.clone()];
        let res = aw!(additional_mirror_to_disk(
            fake.clone(),
            log,
            images.clone(),
            mp,
        ));
        if res.is_err() {
            log.error(&format!(
                "result {:#?}",
                res.as_ref().err().unwrap().to_string()
            ));
        }
        assert_eq!(res.is_err(), true);

        fs::remove_dir_all("./test-artifacts/manifests").expect("should delete test directory");
        fs::remove_dir_all("./test-artifacts/mappings").expect("should delete test directory");
        fs::remove_dir_all("./test-artifacts/blobs-store").expect("should delete test directory");
        fs::remove_dir_all("./test-artifacts/mirror-metadata")
            .expect("should delete test directory");
    }
}
