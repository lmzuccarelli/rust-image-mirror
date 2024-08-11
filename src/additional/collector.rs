use custom_logger::*;
use futures::stream::FuturesUnordered;
use futures::stream::StreamExt;
use mirror_auth::*;
use mirror_copy::{parse_json_manifest_operator, *};
use std::collections::HashMap;
use std::fs;
use std::process;

use crate::api::schema::MirrorImageInfo;
use crate::config::load::*;
use crate::image::utils::*;

// collect all additional images
pub async fn additional_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    skip_manifests_check: bool,
    dry_run: bool,
    additional: Vec<Image>,
) {
    log.hi("additional images collector mode: mirror-to-disk");

    let blobs_dir = dir.clone() + "/blobs-store/";
    let mut image_ref_tracker: Vec<MirrorImageInfo> = Vec::new();
    let mut vec_fslayers: Vec<FsLayer> = vec![];
    let mut fslayers: HashMap<String, Vec<FsLayer>> = HashMap::new();

    // parse the config
    for image in additional.iter() {
        let ir = parse_image(log, image.name.clone());
        log.debug(&format!("image refs {:#?}", ir));

        let token = get_token(log, ir.clone().registry).await;
        if token.is_err() {
            log.error(&format!("{:#?}", token.err().unwrap()));
            process::exit(1);
        }
        // set both url and cache params
        let manifest_url = format!(
            "https://{}/v2/{}/{}/manifests/{}",
            ir.registry, ir.namespace, ir.name, ir.version
        );
        // set up dir to store all manifests
        fs::create_dir_all(&format!(
            "{}/{}",
            dir.clone(),
            "/manifests/additional".to_string()
        ))
        .expect("should create manifests directory");

        let mut manifest_list: String = String::new();
        let working_dir_cache = format!("{}/manifests/additional", dir.clone());
        let mflist_file = format!(
            "{}/{}-{}.json",
            working_dir_cache,
            image.name.clone().replace("/", "-"),
            "list"
        );
        if !skip_manifests_check {
            log.ex(&format!(
                "api call : checking manifest list {:#}",
                ir.registry.clone() + &"/" + &ir.namespace.clone() + "/" + &ir.name.clone()
            ));
            let res = reg_con
                .get_manifest(manifest_url.clone(), token.as_ref().unwrap().to_string())
                .await;

            // manifest check
            if res.is_ok() {
                // write manifestlist to disk
                fs::write(&mflist_file, res.as_ref().unwrap()).expect("write manifest list");
                manifest_list = res.unwrap();
            }
        } else {
            manifest_list =
                fs::read_to_string(mflist_file.clone()).expect("should read manifest list");
        }

        let mem_manifest_list = manifest_list.clone();
        let ml = parse_json_manifestlist(mem_manifest_list.clone());
        if ml.is_ok() {
            for m in ml.unwrap().manifests.iter() {
                let mut ir_url = ir.clone();
                let arch = m.platform.as_ref().unwrap().architecture.to_string();
                ir_url.version = m.digest.as_ref().unwrap().to_string();

                let arch_manifest_json = format!(
                    "{}/manifests/additional/{}-{}.json",
                    dir.clone(),
                    ir_url.version.clone(),
                    arch.clone(),
                );

                if !skip_manifests_check {
                    log.ex(&format!(
                        "api call : checking arch manifest {:#}",
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
                        .get_manifest(mnfst_url.clone(), token.as_ref().unwrap().clone())
                        .await;
                    if res.is_ok() {
                        fs::write(arch_manifest_json.clone(), res.as_ref().unwrap())
                            .expect("unable to write manifest.json file");
                    } else {
                        log.error(&format!(
                            "api call for arch manifest {:?}",
                            res.err().unwrap().to_string().to_lowercase()
                        ));
                    }
                }
            }
        }

        // at this stage we are confident all manifests are on disk (cache)
        // lets verify all related blobs
        let data = fs::read_to_string(&mflist_file);
        if data.is_ok() {
            let manifestlist_mem = parse_json_manifestlist(data.unwrap());
            if manifestlist_mem.is_ok() {
                // read each manifest architecture from disk
                let mlm = manifestlist_mem.as_ref().unwrap();
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
                    let arch_data = fs::read_to_string(arch_manifest_json);
                    if arch_data.is_ok() {
                        let arch_manifest = parse_json_manifest_operator(arch_data.unwrap());
                        if arch_manifest.is_ok() {
                            for l in arch_manifest
                                .as_ref()
                                .unwrap()
                                .layers
                                .as_ref()
                                .unwrap()
                                .iter()
                            {
                                let fsl = FsLayer {
                                    blob_sum: l.digest.clone(),
                                    original_ref: Some(ir.name.clone()),
                                    size: Some(l.size),
                                };
                                vec_fslayers.insert(0, fsl.clone());
                            }
                            let cfg = arch_manifest.unwrap().config.unwrap();
                            let fsl = FsLayer {
                                blob_sum: cfg.digest.clone(),
                                original_ref: Some(ir.name.clone()),
                                size: Some(cfg.size),
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
                        } else {
                            log.error(&format!(
                                "could not parse manifest for architecture {} {:#}",
                                arch.clone(),
                                arch_manifest.err().unwrap().to_string().to_lowercase()
                            ));
                        }
                    } else {
                        log.error(&format!(
                            "could not read manifest for architecture {} {:#}",
                            arch.clone(),
                            arch_data.err().unwrap().to_string().to_lowercase()
                        ));
                    }
                }
                let url = format!(
                    "https://{}/v2/{}/{}/blobs/",
                    ir.registry, ir.namespace, ir.name
                );
                fslayers.insert(url.clone(), vec_fslayers.clone());
            } else {
                log.error(&format!(
                    "could not parse manifest list {:#}",
                    manifestlist_mem.err().unwrap().to_string().to_lowercase()
                ));
            }
        } else {
            log.error(&format!(
                "could not read manifest list {:#}",
                data.err().unwrap().to_string().to_lowercase()
            ));
        }
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs::write(
        dir.clone() + &"/mirror-metadata/additional-image-reference.json",
        serialized_manifest,
    )
    .expect("should write image reference json");

    // if dry run set don't execute blob concurrency section
    if dry_run {
        let mut buf = String::from("");
        let data =
            fs::read_to_string(dir.clone() + &"/mirror-metadata/additional-image-reference.json");
        if data.is_ok() {
            let air = parse_json_metadata(data.unwrap());
            if air.is_ok() {
                for mii in air.unwrap().iter() {
                    if mii.arch == "amd64" || mii.arch == "x86_64" {
                        let src = &format!("{}{}", "docker://", mii.reference);
                        let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                        buf = buf + &format!("{}={}\n", src, dest);
                    }
                }
                fs::write(dir.clone() + "/mappings/additional-mapping.txt", buf)
                    .expect("should write additional-mapping.txt file");
                log.info(&format!(
                    "created additional images mapping file in folder {}",
                    dir.clone() + &"/mappings/",
                ));
            } else {
                log.error(&format!(
                    "parsing additional images metadata file {:#}",
                    air.err().unwrap().to_string().to_lowercase()
                ));
            }
        } else {
            log.error(&format!(
                "reading additional images metadata file {:#}",
                data.err().unwrap().to_string().to_lowercase()
            ));
        }
    } else {
        // we can now get blobs using our cool concurrency model
        // get blobs in batch of 8
        // each future handles get_blobs api call
        // with 8 threads (one per digest)
        let mut futs = FuturesUnordered::new();
        let batch_size = 8;
        for (k, v) in fslayers.iter() {
            // batch the calls
            let hld = k.split("https://").nth(1).unwrap();
            let registry = hld.split("/").nth(0).unwrap();
            log.trace(&format!("url {}", k));
            let token = get_token(log, registry.to_string()).await;
            if token.is_ok() {
                futs.push(reg_con.get_blobs(
                    log,
                    blobs_dir.clone(),
                    k.to_string(),
                    token.as_ref().unwrap().to_string(),
                    v.clone(),
                ));
                if futs.len() >= batch_size {
                    let response = futs.next().await.unwrap();
                    log.debug(&format!(
                        "completed batch of {} {:#?}",
                        batch_size,
                        response.unwrap()
                    ));
                }
            } else {
                log.error(&format!(
                    "token {:#}",
                    token.err().unwrap().to_string().to_lowercase()
                ));
            }
        }
        // Wait for the remaining to finish.
        while let Some(response) = futs.next().await {
            log.debug(&format!("completed rest of batch {:#?}", response.unwrap()));
        }
    }
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;

    #[test]
    fn mirror_to_disk_pass() {
        let _log = &Logging {
            log_level: Level::TRACE,
        };

        // we set up a mock server for the auth-credentials
        let mut server = mockito::Server::new();
        let _url = server.url();

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

        //#[derive(Clone)]
        //struct Fake {}
    }
}
