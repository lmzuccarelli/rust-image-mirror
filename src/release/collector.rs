use crate::api::schema::*;
use crate::auth::credentials::*;
use crate::batch::copy::*;
use crate::index::resolve::*;
use crate::log::logging::*;
use crate::manifests::catalogs::*;

use std::fs;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use walkdir::WalkDir;

// collect all operator images
pub async fn release_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    release: String,
) {
    log.hi("release collector mode: mirrorToDisk");

    // parse the config
    let img_ref = parse_release_image_index(log, release);
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
    let sub_dir = dir.clone() + "blobs-store";
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
            // re-create the cache directory
            let mut builder = DirBuilder::new();
            builder.mode(0o777);
            builder
                .create(&working_dir_cache)
                .expect("unable to create directory");
        }
        untar_layers(
            log,
            sub_dir.clone(),
            working_dir_cache.clone(),
            res_manifest_in_mem.fs_layers,
        )
        .await;
        log.hi("completed untar of layers");
        // original !exists end }

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
        let imgs = get_release_json(config_dir + "/image-references");
        log.trace(&format!(
            "images from release-manifests/image-reference {:#?}",
            imgs
        ));

        // iterate through all the release image-references
        let release_dir =
            dir.clone() + "/" + &img_ref.clone().name + "/" + &img_ref.clone().version + "/";
        for img in imgs.spec.tags.iter() {
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
    release: String,
) -> String {
    let release_dir = dir.clone() + &get_dir_from_isc(release);
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
                dir.clone(), //String::from("./working-dir/"),
                String::from("ocp-release"),
                destination_url.clone(),
                String::from(""),
                manifest.clone(),
            )
            .await;
    }
    String::from("ok")
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

fn get_release_json(dir: String) -> ReleaseSchema {
    let data = fs::read_to_string(&dir).expect("should read release-reference json file");
    let release_schema = parse_json_release_imagereference(data).unwrap();
    release_schema
}

fn get_release_manifest(dir: String) -> Manifest {
    let data = fs::read_to_string(&dir).expect("should read release-operator-manifest json file");
    let release_manifest = parse_json_manifest_operator(data).unwrap();
    release_manifest
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

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use crate::error::handler::MirrorError;
    use async_trait::async_trait;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn get_related_images_from_catalog_with_channel_pass() {
        let _log = &Logging {
            log_level: Level::TRACE,
        };
        let ic = IncludeChannel {
            name: String::from("alpha"),
            min_version: None,
            max_version: None,
            min_bundle: None,
        };
        let ics = vec![ic];
        let pkg = Package {
            name: String::from("some-operator"),
            channels: Some(ics),
            min_version: None,
            max_version: None,
            min_bundle: None,
        };
        let _pkgs = vec![pkg];

        let ir1 = RelatedImage {
            name: String::from("controller"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:d7bc364512178c36671d8a4b5a76cf7cb10f8e56997106187b0fe1f032670ece"),
        };
        let ir2 = RelatedImage {
            name: String::from(""),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:50b9402635dd4b312a86bed05dcdbda8c00120d3789ec2e9b527045100b3bdb4"),
        };
        let ir3 = RelatedImage {
            name: String::from("aws-load-balancer-rhel8-operator-95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d-annotation"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"),
        };
        let ir4 = RelatedImage {
            name: String::from("manager"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"),
        };
        let ir5 = RelatedImage {
            name: String::from("kube-rbac-proxy"),
            image: String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:3658954f199040b0f244945c94955f794ee68008657421002e1b32962e7c30fc"),
        };
        let ri_vec = vec![ir1, ir2, ir3, ir4, ir5];
        let _wrapper = RelatedImageWrapper {
            name: String::from("test"),
            images: ri_vec,
            channel: String::from("alpha"),
        };
    }

    #[test]
    fn get_related_images_from_catalog_no_channel_pass() {
        let _log = &Logging {
            log_level: Level::INFO,
        };
        let pkg = Package {
            name: String::from("some-operator"),
            channels: None,
            min_version: None,
            max_version: None,
            min_bundle: None,
        };
        let _pkgs = vec![pkg];

        let ir1 = RelatedImage {
            name: String::from("controller"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92"),
        };
        let ir2 = RelatedImage {
            name: String::from(""),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d"),
        };
        let ir3 = RelatedImage {
            name: String::from("aws-load-balancer-rhel8-operator-95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d-annotation"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"),
        };
        let ir4 = RelatedImage {
            name: String::from("manager"),
            image: String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"),
        };
        let ir5 = RelatedImage {
            name: String::from("kube-rbac-proxy"),
            image: String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6"),
        };
        let ri_vec = vec![ir1, ir2, ir3, ir4, ir5];
        let _wrapper = RelatedImageWrapper {
            name: String::from("test"),
            images: ri_vec,
            channel: String::from("stable-v1"),
        };
    }

    #[test]
    fn mirror_to_disk_pass() {
        let log = &Logging {
            log_level: Level::TRACE,
        };

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

        let pkg = Package {
            name: String::from("some-operator"),
            channels: None,
            min_version: None,
            max_version: None,
            min_bundle: None,
        };

        let pkgs = vec![pkg];
        let _op = Operator {
            catalog: String::from(url.replace("http://", "") + "/test/test-index-operator:v1.0"),
            packages: Some(pkgs),
        };

        #[derive(Clone)]
        struct Fake {}

        #[async_trait]
        impl RegistryInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, Box<dyn std::error::Error>> {
                let mut content = String::from("");
                // simulate api calls
                if url.contains("test-release-operator") {
                    content = fs::read_to_string(
                        "./test-artifacts/simulate-api-call/manifest-release-index.json",
                    )
                    .expect("should read release-operator manifest file")
                }

                if url.contains("ade18f2994669ebeb870b3b545f8b48574da9fc26ea24341dd1c16faac9994a0")
                {
                    content = fs::read_to_string(
                        "test-artifacts/simulate-api-call/manifest-release.json",
                    )
                    .expect("should read test-release-operator manifest file");
                }
                Ok(content)
            }

            async fn get_blobs(
                &self,
                log: &Logging,
                _dir: String,
                url: String,
                _token: String,
                _layers: Vec<FsLayer>,
            ) -> Result<String, Box<dyn std::error::Error>> {
                log.info("testing get blobs in fake test");
                if url.contains("test-release-operator/blobs") {
                    fs::create_dir_all("./test-artifacts/blobs-store/ac/")
                        .expect("make dir test-artifacts/blob-store");
                    fs::copy(
                        "./test-artifacts/raw-tar-files/ac/ac202bb709d9c0744e8fd6f3ed3c5c57eec4c7b16caeadac7b4b323f94f5809e",
                        "./test-artifacts/blobs-store/ac/ac202bb709d9c0744e8fd6f3ed3c5c57eec4c7b16caeadac7b4b323f94f5809e",
                    )
                    .expect("should copy blob file");
                }
                Ok(String::from("test"))
            }

            async fn push_image(
                &self,
                log: &Logging,
                _dir: String,
                _subdir: String,
                _url: String,
                _token: String,
                _manifest: Manifest,
            ) -> Result<String, MirrorError> {
                log.info("testing logging in fake test");
                Ok(String::from("test"))
            }
        }

        let fake = Fake {};

        // do a cleanup of test-artifacts

        let exists = Path::new("./test-artifacts/blobs-store/ac").exists();
        if exists {
            fs::remove_dir_all("./test-artifacts/blobs-store/ac")
                .expect("should delete test artifact blob-store");
        }
        let exists = Path::new("./test-artifacts/test-release-operator/v1.0/cache/ac202b").exists();
        if exists {
            fs::remove_dir_all("./test-artifacts/test-release-operator/v1.0/cache/ac202b")
                .expect("should delete test artifact cache");
        }
        let exists =
            Path::new("./test-artifacts/test-release-operator/v1.0/manifest.json").exists();
        if exists {
            fs::remove_file("./test-artifacts/test-release-operator/v1.0/manifest.json")
                .expect("should delete test release operator manifest.json");
        }

        aw!(release_mirror_to_disk(
            fake.clone(),
            log,
            String::from("./test-artifacts/"),
            String::from(url.replace("http://", "") + "/test/test-release-operator:v1.0")
        ));

        aw!(release_disk_to_mirror(
            fake.clone(),
            log,
            String::from("./test-artifacts/"),
            String::from(url.replace("http://", "") + "/test/"),
            String::from("/test/test-release-operator:v1.0")
        ));
    }
}
