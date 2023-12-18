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
pub async fn operator_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    operators: Vec<Operator>,
) {
    log.hi("operator collector mode: mirrorToDisk");

    // parse the config - iterate through each catalog
    let img_ref = parse_image_index(log, operators);
    log.debug(&format!("image refs {:#?}", img_ref));

    for ir in img_ref {
        let manifest_json = get_manifest_json_file(
            // ./working-dir
            dir.clone(),
            ir.name.clone(),
            ir.version.clone(),
        );
        log.trace(&format!("manifest json file {}", manifest_json));
        let token = get_token(log, ir.registry.clone()).await;
        // use token to get manifest
        let manifest_url = get_image_manifest_url(ir.clone());
        let manifest = reg_con
            .get_manifest(manifest_url.clone(), token.clone())
            .await
            .unwrap();

        // create the full path
        let manifest_dir = manifest_json.split("manifest.json").nth(0).unwrap();
        log.info(&format!("manifest directory {}", manifest_dir));
        fs::create_dir_all(manifest_dir).expect("unable to create directory manifest directory");
        let mut exists = Path::new(&manifest_json).exists();
        let res_manifest_in_mem = parse_json_manifest(manifest.clone()).unwrap();
        let working_dir_cache = get_cache_dir(dir.clone(), ir.name.clone(), ir.version.clone());
        let sub_dir = dir.clone() + "/blobs-store/";
        if exists {
            let manifest_on_disk = fs::read_to_string(&manifest_json).unwrap();
            let res_manifest_on_disk = parse_json_manifest(manifest_on_disk).unwrap();
            if res_manifest_on_disk != res_manifest_in_mem {
                exists = false;
            }
        }
        if !exists {
            log.info("detected change in index manifest");
            fs::write(manifest_json, manifest.clone())
                .expect("unable to write (index) manifest.json file");
            let blobs_url = get_blobs_url(ir.clone());
            // use a concurrent process to get related blobs
            reg_con
                .get_blobs(
                    log,
                    sub_dir.clone(),
                    blobs_url,
                    token.clone(),
                    res_manifest_in_mem.fs_layers.clone(),
                )
                .await;
            log.info("completed image index download");
            // detected a change so clean the dir contents
            rm_rf::remove(&working_dir_cache).expect("should delete current untarred cache");
            // re-create the cache directory
            let mut builder = DirBuilder::new();
            builder.mode(0o755);
            builder
                .create(&working_dir_cache)
                .expect("unable to create directory");
            untar_layers(
                log,
                sub_dir.clone(),
                working_dir_cache.clone(),
                res_manifest_in_mem.fs_layers,
            )
            .await;
            log.hi("completed untar of layers");
        }

        // find the directory 'configs'
        // TODO if new blobs are downloaded the config dir could be in another blob
        let config_dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
        log.mid(&format!(
            "full path for directory 'configs' {} ",
            &config_dir
        ));

        let wrappers = get_related_images_from_catalog(
            log,
            config_dir,
            ir.packages.clone().expect("should have packages"),
        );
        log.debug(&format!("images from catalog for {:#?}", ir.packages));

        // iterate through all the related images
        for wrapper in wrappers.iter() {
            for imgs in wrapper.images.iter() {
                // first check if the manifest exists
                let op_name = get_operator_name(
                    wrapper.name.clone(),
                    imgs.image.clone(),
                    wrapper.channel.clone(),
                );
                log.info(&format!("manifest for operator {:#?}", op_name));
                let op_dir =
                    get_operator_manifest_json_dir(dir.clone(), &ir.name, &ir.version, &op_name);
                fs::create_dir_all(op_dir.clone()).expect("should create full operator path");
                log.debug(&format!("operator manifest path {:#?}", op_dir));
                //let file = op_dir.clone() + "/manifest.json";
                //if !Path::new(&file).exists() {
                let manifest_url = get_manifest_url(imgs.image.clone());
                log.trace(&format!("manifest url {:#?}", manifest_url));
                // use the RegistryInterface to make the call
                let manifest = reg_con
                    .get_manifest(manifest_url.clone(), token.clone())
                    .await
                    .unwrap();

                log.trace(&format!("manifest contents {:#?}", manifest));
                // check if the manifest is of type list
                let manifest_list = parse_json_manifestlist(manifest.clone());
                log.trace(&format!("manifest list {:#?}", manifest_list));
                fs::create_dir_all(&op_dir).expect("unable to create operator manifest directory");
                let mut fslayers = Vec::new();
                if manifest_list.is_ok() {
                    let ml = manifest_list.unwrap().clone();
                    log.trace(&format!("manifest list detected {:#?}", ml));
                    if ml.media_type == "application/vnd.docker.distribution.manifest.list.v2+json"
                    {
                        fs::write(op_dir.clone() + "/manifest-list.json", manifest.clone())
                            .expect("unable to write file");
                        // look for the digest
                        // loop through each manifest
                        for mf in ml.manifests.iter() {
                            let sub_manifest_url = get_manifest_url_by_digest(
                                imgs.image.clone(),
                                mf.digest.clone().unwrap(),
                            );
                            log.trace(&format!("sub manifest url {:#?}", sub_manifest_url.clone()));
                            // use the RegistryInterface to make the api call
                            let local_manifest = reg_con
                                .get_manifest(sub_manifest_url.clone(), token.clone())
                                .await
                                .unwrap();

                            fs::write(
                                op_dir.clone()
                                    + "/manifest-"
                                    + &mf.platform.clone().unwrap().architecture
                                    + ".json",
                                local_manifest.clone(),
                            )
                            .expect("unable to write file");
                            log.trace(&format!(
                                "local manifest (from sub manifest url) {:#?}",
                                local_manifest.clone()
                            ));
                            // convert op_manifest.layer to FsLayer and add it to the collection
                            let op_manifest =
                                parse_json_manifest_operator(local_manifest.clone()).unwrap();
                            for layer in op_manifest.layers.unwrap().iter() {
                                let fslayer = FsLayer {
                                    blob_sum: layer.digest.clone(),
                                    original_ref: Some(imgs.image.clone()),
                                    size: Some(layer.size),
                                };
                                fslayers.insert(0, fslayer);
                            }
                            // add configs
                            let config = op_manifest.config.unwrap();
                            let cfg = FsLayer {
                                blob_sum: config.digest,
                                original_ref: Some(imgs.image.clone()),
                                size: Some(config.size),
                            };
                            fslayers.insert(0, cfg);
                        }
                    }
                } else {
                    fs::write(op_dir.clone() + "/manifest.json", manifest.clone())
                        .expect("unable to write file");
                    // now download each related images blobs
                    log.debug(&format!("manifest dir {:#?}", op_dir));
                    let op_manifest = parse_json_manifest_operator(manifest.clone()).unwrap();
                    log.trace(&format!("op_manifest {:#?}", op_manifest));
                    // convert op_manifest.layer to FsLayer
                    for layer in op_manifest.layers.unwrap().iter() {
                        let fslayer = FsLayer {
                            blob_sum: layer.digest.clone(),
                            original_ref: Some(imgs.image.clone()),
                            size: Some(layer.size),
                        };
                        fslayers.insert(0, fslayer);
                    }
                    // add configs
                    let config = op_manifest.config.unwrap();
                    let cfg = FsLayer {
                        blob_sum: config.digest,
                        original_ref: Some(imgs.image.clone()),
                        size: Some(config.size),
                    };
                    fslayers.insert(0, cfg);
                }
                let op_url = get_blobs_url_by_string(imgs.image.clone());
                reg_con
                    .get_blobs(
                        log,
                        sub_dir.clone(),
                        op_url,
                        token.clone(),
                        fslayers.clone(),
                    )
                    .await;
            }
        }
    }
}

pub async fn operator_disk_to_mirror<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    destination_url: String,
    operators: Vec<Operator>,
) -> String {
    // read isc catalogs, packages
    // read all manifests and blobs from disk
    // build the list
    // call push_blobs
    let mut mirror_manifests = vec![];
    log.hi("operator collector mode: diskToMirror");
    for op in operators.iter() {
        log.info(&format!("catalog {:#?} ", &op.catalog));
        for pkg in op.packages.clone().unwrap().iter() {
            log.info(&format!("packages {:#?} ", pkg));
            let ir = get_registry_details(&op.catalog);
            // iterate through each directory in
            // does it match with the pkg name
            // if yes then lets see if channels are set
            // with this info we can open the manifest to get all layers
            let manifest_dir =
                get_operator_manifest_json_dir(dir.clone(), &ir.name, &ir.version, &pkg.name);
            if pkg.channels.is_some() {
                for channel in pkg.channels.clone().unwrap().iter() {
                    log.debug(&format!(
                        "adding manifest {:#?}",
                        manifest_dir.clone() + &"/" + &channel.name
                    ));
                    let check_dir = manifest_dir.clone() + &"/" + &channel.name;
                    let am = get_all_assosciated_manifests(log, check_dir.clone());
                    mirror_manifests.insert(0, am.clone());
                }
            } else {
                // this means we take all channels listed
                log.info("no channel/s set");
                let paths = fs::read_dir(&manifest_dir).unwrap();
                for path in paths {
                    let entry = path.unwrap();
                    let file = entry.path();
                    // we have the channel
                    if file.is_dir() {
                        log.debug(&format!("adding manifest {:#?}", &file));
                        let hld = file.clone().into_os_string().into_string().unwrap();
                        let am = get_all_assosciated_manifests(log, hld);
                        mirror_manifests.insert(0, am.clone());
                    }
                }
            }
        }
    }
    // we now have all the relevant manifests
    log.debug(&format!(
        "list all relevant manifests {:#?}",
        mirror_manifests
    ));

    // using map and collect are not async
    for mm in mirror_manifests.iter() {
        for x in mm.iter() {
            // we can infer some info from the manifest
            let binding = x.to_string();
            let rd = get_registry_details_from_manifest(binding.clone());
            log.trace(&format!("metadata for manifest {:#?}", rd));
            let manifest = get_manifest(binding);
            let _res = reg_con
                .push_image(
                    log,
                    rd.sub_component,
                    destination_url.clone(),
                    String::from(""),
                    manifest.clone(),
                )
                .await;
        }
    }
    String::from("ok")
}

fn get_manifest(dir: String) -> Manifest {
    let data = fs::read_to_string(&dir).expect("should read various arch manifest files");
    let manifest = parse_json_manifest_operator(data).unwrap();
    manifest
}

fn get_all_assosciated_manifests(log: &Logging, dir: String) -> Vec<String> {
    let mut vec_manifests: Vec<String> = vec![];
    let result = WalkDir::new(&dir);
    for file in result.into_iter().filter_map(|file| file.ok()) {
        if file.metadata().unwrap().is_file() & !file.path().display().to_string().contains("list")
        {
            log.debug(&format!(
                "assosciated manifest found {:#?}",
                file.path().display().to_string()
            ));
            vec_manifests.insert(0, file.path().display().to_string());
        }
    }
    vec_manifests
}

fn get_related_images_from_catalog(
    log: &Logging,
    dir: String,
    packages: Vec<Package>,
) -> Vec<RelatedImageWrapper> {
    let mut bundle_name = String::from("");
    let mut related_image_wrapper = Vec::new();

    for pkg in packages {
        let dc_json = read_operator_catalog(dir.to_string() + &"/".to_string() + &pkg.name)
            .unwrap()
            .clone();
        let dc: Vec<DeclarativeConfig> = serde_json::from_value(dc_json.clone()).unwrap();

        log.lo(&format!(
            "default channel {:?} for operator {} ",
            dc[0].default_channel, pkg.name
        ));

        // first check if channels are valid
        if pkg.channels.is_some() {
            // iterate through each channel
            for chn in pkg.channels.unwrap().iter() {
                for obj in dc.iter() {
                    if obj.schema == "olm.channel" {
                        log.trace(&format!("channels compare {:#?} {:#?}", chn.name, obj.name));
                        if chn.name == obj.name {
                            // get the entries object[0] which is the bundle we are after
                            // TODO: could be more than one entry
                            let entries: Vec<ChannelEntry> = match obj.entries.clone() {
                                Some(val) => val,
                                None => vec![],
                            };
                            bundle_name = entries[0].name.clone();
                        }
                    }
                    if obj.schema == "olm.bundle" {
                        if bundle_name == obj.name {
                            log.trace(&format!("bundle {:#?} {}", obj.related_images, obj.name));
                            let wrapper = RelatedImageWrapper {
                                name: pkg.name.clone(),
                                images: obj.related_images.clone().unwrap(),
                                channel: chn.name.clone(),
                            };
                            related_image_wrapper.insert(0, wrapper);
                        }
                    }
                }
            }
        } else {
            // case when we don't have channels in the imageset config
            // we look for default channel
            for obj in dc.iter() {
                if obj.schema == "olm.bundle" {
                    if bundle_name == obj.name {
                        log.trace(&format!("bundle {:#?} {}", obj.related_images, obj.name));
                        let wrapper = RelatedImageWrapper {
                            name: pkg.name.clone(),
                            images: obj.related_images.clone().unwrap(),
                            channel: dc[0].default_channel.clone().unwrap(),
                        };
                        related_image_wrapper.insert(0, wrapper);
                    }
                }
                if obj.schema == "olm.channel" {
                    if obj.name == dc[0].default_channel.clone().unwrap() {
                        log.trace(&format!("channel info {:#?} {}", obj.entries, obj.name));
                        let entries: Vec<ChannelEntry> = match obj.entries.clone() {
                            Some(val) => val,
                            None => vec![],
                        };
                        bundle_name = entries[0].name.clone();
                    }
                }
            }
        }
    }
    related_image_wrapper
}

// construct the operator namespace and name
fn get_operator_name(operator_name: String, img: String, channel: String) -> String {
    let mut parts = img.split("/");
    let _ = parts.nth(0).unwrap();
    let ns = parts.nth(0).unwrap();
    let name = parts.nth(0).unwrap();
    let mut op_name = name.split("@");
    operator_name.to_string()
        + "/"
        + &channel.clone()
        + "/"
        + &ns.to_string()
        + "/"
        + &op_name.nth(0).unwrap().to_owned()
}

// utility functions - get_operator_manifest_json_dir
fn get_operator_manifest_json_dir(
    dir: String,
    name: &str,
    version: &str,
    operator: &str,
) -> String {
    // ./working-dir
    let mut file = dir.clone();
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/operators/");
    file.push_str(&operator);
    file
}

fn get_registry_details(reg: &str) -> ImageReference {
    let mut ver = reg.split(":");
    let mut hld = ver.nth(0).unwrap().split("/");
    let pkg = Package {
        name: String::from(""),
        channels: None,
        min_version: None,
        max_version: None,
        min_bundle: None,
    };
    let vec_pkg = vec![pkg];
    let ir = ImageReference {
        registry: hld.nth(0).unwrap().to_string(),
        namespace: hld.nth(0).unwrap().to_string(),
        name: hld.nth(0).unwrap().to_string(),
        version: ver.nth(0).unwrap().to_string(),
        packages: Some(vec_pkg),
    };
    ir
}

fn get_registry_details_from_manifest(name: String) -> MirrorManifest {
    let res = name.split("/");
    let collection = res.clone().collect::<Vec<&str>>();
    let mm = MirrorManifest {
        registry: collection[1].to_string(),
        namespace: collection[2].to_string(),
        name: String::from(""),
        version: collection[3].to_string(),
        component: collection[5].to_string(),
        channel: collection[6].to_string(),
        sub_component: collection[7].to_string() + &"/" + collection[8],
        manifest_file: collection[9].to_string(),
    };
    mm
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

        let ops = vec![op];
        aw!(operator_mirror_to_disk(
            fake,
            log,
            String::from("test-artifacts/"),
            ops
        ));
    }
}
