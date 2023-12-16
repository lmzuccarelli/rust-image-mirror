use crate::api::schema::*;
use crate::auth::credentials::*;
use crate::batch::copy::*;
use crate::index::resolve::*;
use crate::log::logging::*;
use crate::manifests::catalogs::*;

use std::fs;

// collect all operator images
pub async fn release_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    release: String,
) {
    log.info("release collector mode: mirrorToDisk");

    // parse the config - iterate through each release image index
    let img_ref = parse_release_image_index(log, release);
    log.debug(&format!("image refs {:#?}", img_ref));

    let manifest_json = get_manifest_json_file(
        // ./working-dir
        dir.clone(),
        img_ref.name.clone(),
        img_ref.version.clone(),
    );
    log.trace(&format!("manifest json file {}", manifest_json));
    let token = get_token(log, img_ref.clone().registry).await;
    let manifest_url = get_image_manifest_url(img_ref.clone());
    log.trace(&format!("manifest url {}", manifest_url));
    let manifest = reg_con
        .get_manifest(manifest_url.clone(), token.clone())
        .await
        .unwrap();

    // create the full path
    // TODO:
    let manifest_dir = manifest_json.split("manifest.json").nth(0).unwrap();
    log.info(&format!("manifest directory {}", manifest_dir));
    fs::create_dir_all(manifest_dir).expect("unable to create directory manifest directory");
    fs::write(manifest_json, manifest.clone()).expect("unable to write (index) manifest.json file");
    let res = parse_json_manifest(manifest).unwrap();
    let blobs_url = get_blobs_url(img_ref.clone());

    // use a concurrent process to get related blobs
    let sub_dir = dir.clone() + "/blobs-store/";
    reg_con
        .get_blobs(
            log,
            sub_dir.clone(),
            blobs_url,
            token.clone(),
            res.fs_layers.clone(),
        )
        .await;
    log.info("completed image index download");

    let working_dir_cache =
        get_cache_dir(dir.clone(), img_ref.name.clone(), img_ref.version.clone());
    // create the cache directory
    fs::create_dir_all(&working_dir_cache).expect("unable to create directory");
    untar_layers(
        log,
        sub_dir.clone(),
        working_dir_cache.clone(),
        res.fs_layers,
    )
    .await;
    log.hi("completed untar of layers");

    // find the directory 'release-manifests'
    let config_dir = find_dir(
        log,
        working_dir_cache.clone(),
        "release-manifests".to_string(),
    )
    .await;
    log.mid(&format!(
        "full path for directory 'configs' {} ",
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
        log.trace(&format!("manifest url {:#?}", manifest_url));

        // use the RegistryInterface to make the call
        let manifest = reg_con
            .get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        log.info(&format!("writing manifest for {:#?}", img.name.clone()));
        log.trace(&format!("manifest contents {:#?}", manifest));

        let mut fslayers = Vec::new();
        let op_manifest = parse_json_manifest_operator(manifest.clone()).unwrap();
        let origin_tmp = img.from.name.split("@");
        let origin = origin_tmp.clone().nth(0).unwrap();

        // convert op_manifest.layer to FsLayer
        for layer in op_manifest.layers.unwrap().iter() {
            let fslayer = FsLayer {
                blob_sum: layer.digest.clone(),
                original_ref: Some(origin.to_string()),
                //result: Some(String::from("")),
            };
            fslayers.insert(0, fslayer);
        }
        // add configs
        let cfg = FsLayer {
            blob_sum: op_manifest.config.unwrap().digest,
            original_ref: Some(origin.to_string()),
            //result: Some(String::from("")),
        };
        fslayers.insert(0, cfg);
        let op_url = get_blobs_url_by_string(img.from.name.clone());
        let blobs_dir = "./working-dir/blobs-store/".to_string();
        log.trace(&format!("blobs_url {}", op_url));
        log.trace(&format!("fslayer for {} {:#?}", img.name, fslayers));

        reg_con
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

pub async fn release_disk_to_mirror<T: RegistryInterface>(
    _reg_con: T,
    _log: &Logging,
    _dir: String,
    _destination_url: String,
    _operators: String,
) -> String {
    String::from("ok")
}

fn get_release_json(dir: String) -> ReleaseSchema {
    let data = fs::read_to_string(&dir).expect("should read release-reference json file");
    let release_schema = parse_json_release_imagereference(data).unwrap();
    release_schema
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use async_trait::async_trait;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]
    fn get_operator_manfifest_json_dir_pass() {
        let res = get_operator_manifest_json_dir(
            String::from("test-artifacts/"),
            &"test-index",
            &"v1.0",
            &"some-operator",
        );
        assert_eq!(
            res,
            String::from("test-artifacts/test-index/v1.0/operators/some-operator")
        );
    }

    #[test]
    fn get_operator_name_pass() {
        let res = get_operator_name(
            String::from("registry"),
            String::from("test.registry.io/test/some-operator@sha256:1234567890"),
            String::from("channel"),
        );
        assert_eq!(res, String::from("registry/channel/test/some-operator"));
    }

    #[test]
    fn get_related_images_from_catalog_with_channel_pass() {
        let log = &Logging {
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
        let pkgs = vec![pkg];

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
        let wrapper = RelatedImageWrapper {
            name: String::from("test"),
            images: ri_vec,
            channel: String::from("alpha"),
        };
        let wrapper_vec = vec![wrapper];
        let res = get_related_images_from_catalog(
            log,
            String::from("test-artifacts/test-index-operator/v1.0/cache/b4385e/configs/"),
            pkgs,
        );
        log.trace(&format!("results {:#?}", res));
        let matching = res
            .iter()
            .zip(&wrapper_vec)
            .filter(|&(res, wrapper)| res.images.len() == wrapper.images.len())
            .count();
        assert_eq!(matching, 1);
        for x in res.iter() {
            assert_eq!(x.images[0].image, String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:d7bc364512178c36671d8a4b5a76cf7cb10f8e56997106187b0fe1f032670ece"));
            assert_eq!(x.images[1].image, String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:50b9402635dd4b312a86bed05dcdbda8c00120d3789ec2e9b527045100b3bdb4"));
            assert_eq!(x.images[2].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"));
            assert_eq!(x.images[3].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:95c45fae0ca9e9bee0fa2c13652634e726d8133e4e3009b363fcae6814b3461d"));
            assert_eq!(x.images[4].image, String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:3658954f199040b0f244945c94955f794ee68008657421002e1b32962e7c30fc"));
        }
    }

    #[test]
    fn get_related_images_from_catalog_no_channel_pass() {
        let log = &Logging {
            log_level: Level::INFO,
        };
        let pkg = Package {
            name: String::from("some-operator"),
            channels: None,
            min_version: None,
            max_version: None,
            min_bundle: None,
        };
        let pkgs = vec![pkg];

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
        let wrapper = RelatedImageWrapper {
            name: String::from("test"),
            images: ri_vec,
            channel: String::from("stable-v1"),
        };
        let wrapper_vec = vec![wrapper];
        let res = get_related_images_from_catalog(
            log,
            String::from("test-artifacts/test-index-operator/v1.0/cache/b4385e/configs/"),
            pkgs,
        );
        log.trace(&format!("results {:#?}", res));
        let matching = res
            .iter()
            .zip(&wrapper_vec)
            .filter(|&(res, wrapper)| res.images.len() == wrapper.images.len())
            .count();
        assert_eq!(matching, 1);
        for x in res.iter() {
            assert_eq!(x.images[0].image, String::from("registry.redhat.io/albo/aws-load-balancer-controller-rhel8@sha256:cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92"));
            assert_eq!(x.images[1].image, String::from("registry.redhat.io/albo/aws-load-balancer-operator-bundle@sha256:d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d"));
            assert_eq!(x.images[2].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"));
            assert_eq!(x.images[3].image, String::from("registry.redhat.io/albo/aws-load-balancer-rhel8-operator@sha256:cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0"));
            assert_eq!(x.images[4].image, String::from("registry.redhat.io/openshift4/ose-kube-rbac-proxy@sha256:422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6"));
        }
    }

    #[test]
    fn mirror_to_disk_pass() {
        let log = &Logging {
            log_level: Level::DEBUG,
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
        let op = Operator {
            catalog: String::from("test.registry.io/test/test-index-operator:v1.0"),
            packages: Some(pkgs),
        };

        struct Fake {}

        #[async_trait]
        impl RegistryInterface for Fake {
            async fn get_manifest(
                &self,
                url: String,
                _token: String,
            ) -> Result<String, Box<dyn std::error::Error>> {
                let mut content = String::from("");

                if url.contains("test-index-operator") {
                    content =
                        fs::read_to_string("test-artifacts/test-index-operator/v1.0/manifest.json")
                            .expect("should read operator-index manifest file")
                }
                if url.contains("cad8f6380b4dd4e1396dafcd7dfbf0f405aa10e4ae36214f849e6a77e6210d92")
                {
                    content =
                        fs::read_to_string("test-artifacts/simulate-api-call/manifest-list.json")
                            .expect("should read test (albo) controller manifest-list file");
                }
                if url.contains("75012e910726992f70c892b11e50e409852501c64903fa05fa68d89172546d5d")
                    | url.contains(
                        "5e03f571c5993f0853a910b7c0cab44ec0e451b94a9677ed82e921b54a4b735a",
                    )
                {
                    content =
                        fs::read_to_string("test-artifacts/simulate-api-call/manifest-amd64.json")
                            .expect("should read test (albo) controller manifest-am64 file");
                }
                if url.contains("d4d65d0d7c249d076da74da22296280ddef534da2bf54efb9e46d2bd7b9a602d")
                {
                    content = fs::read_to_string("test-artifacts/simulate-api-call/manifest.json")
                        .expect("should read test (albo) bundle manifest file");
                }
                if url.contains("cbb31de2108b57172409cede667fa24d68d635ac3cc6db4af6e9b6f9dd1c5cd0")
                {
                    content = fs::read_to_string(
                        "test-artifacts/simulate-api-call/manifest-amd64-operator.json",
                    )
                    .expect("should read test (albo) operator manifest file");
                }
                if url.contains("422e4fbe1ed81c79084f43a826dc0674510a7ff578e62b4ddda119ed3266d0b6")
                {
                    content = fs::read_to_string(
                        "test-artifacts/simulate-api-call/manifest-amd64-kube.json",
                    )
                    .expect("should read test (openshift) kube-proxy manifest file");
                }

                Ok(content)
            }

            async fn get_blobs(
                &self,
                log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _layers: Vec<FsLayer>,
            ) -> String {
                log.info("testing logging in fake test");
                String::from("test")
            }

            async fn push_image(
                &self,
                log: &Logging,
                _dir: String,
                _url: String,
                _token: String,
                _manifest: Manifest,
            ) -> String {
                log.info("testing logging in fake test");
                String::from("test")
            }
        }

        let fake = Fake {};

        aw!(release_mirror_to_disk(
            fake,
            log,
            String::from("test-artifacts/"),
            String::from("test")
        ));
    }
}
