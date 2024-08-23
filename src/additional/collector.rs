use custom_logger::*;
use mirror_auth::*;
use mirror_copy::{parse_json_manifest_operator, *};
use std::collections::HashMap;
use std::fs;
use std::process;

use crate::api::schema::MirrorImageInfo;
use crate::batch::worker::execute_batch;
use crate::config::load::*;
use crate::error::handler::MirrorError;
use crate::image::utils::*;

// collect all additional images
pub async fn additional_mirror_to_disk<T: RegistryInterface>(
    reg_con: T,
    log: &Logging,
    dir: String,
    skip_manifests_check: bool,
    dry_run: bool,
    additional: Vec<Image>,
    vec_arch: Vec<&str>,
    verify_blobs: bool,
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
        fs_handler(
            format!("{}/{}", dir.clone(), "/manifests/additional"),
            "create_dir",
            None,
        )?;

        let mut manifest_list: String = String::new();
        let working_dir_cache = format!("{}/manifests/additional", dir.clone());
        let mflist_file = format!(
            "{}/{}-{}.json",
            working_dir_cache,
            image.name.clone().replace("/", "-"),
            "list"
        );
        if !skip_manifests_check {
            log.mid(&format!(
                "api call manifest list for {:#}",
                ir.registry.clone() + &"/" + &ir.namespace.clone() + "/" + &ir.name.clone()
            ));
            let res = reg_con
                .get_manifest(manifest_url.clone(), token.as_ref().unwrap().to_string())
                .await;

            // manifest check
            if res.is_ok() {
                //write manifestlist to disk
                fs_handler(
                    mflist_file.clone(),
                    "write",
                    Some(res.as_ref().unwrap().to_string()),
                )?;
                manifest_list = res.unwrap();
            }
        } else {
            let res = fs::read_to_string(mflist_file.clone());
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "reading manifest from diskt {}",
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            } else {
                manifest_list = res.unwrap();
            }
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

                if !skip_manifests_check && !dry_run {
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
                        .get_manifest(mnfst_url.clone(), token.as_ref().unwrap().clone())
                        .await;
                    if res.is_ok() {
                        fs_handler(
                            arch_manifest_json.clone(),
                            "write",
                            Some(res.as_ref().unwrap().to_string()),
                        )?;
                    } else {
                        let err = MirrorError::new(&format!(
                            "api call for arch manifest {}",
                            res.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
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
                                    number: None,
                                };
                                vec_fslayers.insert(0, fsl.clone());
                            }
                            let cfg = arch_manifest.unwrap().config.unwrap();
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
                        } else {
                            let err = MirrorError::new(&format!(
                                "parsing manifest for arch {}",
                                arch_manifest.err().unwrap().to_string().to_lowercase()
                            ));
                            return Err(err);
                        }
                    } else {
                        let err = MirrorError::new(&format!(
                            "reading manifest for arch {}",
                            arch_data.err().unwrap().to_string().to_lowercase()
                        ));
                        return Err(err);
                    }
                }
                let url = format!(
                    "https://{}/v2/{}/{}/blobs/",
                    ir.registry, ir.namespace, ir.name
                );
                fslayers.insert(url.clone(), vec_fslayers.clone());
            } else {
                let err = MirrorError::new(&format!(
                    "parsing manifest list {}",
                    manifestlist_mem.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            let err = MirrorError::new(&format!(
                "reading manifest list {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    }

    image_ref_tracker.sort_by_key(|a| a.name.clone());
    let serialized_manifest = serde_json::to_string(&image_ref_tracker.clone()).unwrap();
    fs_handler(
        dir.clone() + &"/mirror-metadata/additional-image-reference.json",
        "write",
        Some(serialized_manifest),
    )?;

    // if dry run set don't execute blob concurrency section
    if dry_run {
        let mut buf = String::from("");
        let data =
            fs::read_to_string(dir.clone() + &"/mirror-metadata/additional-image-reference.json");
        if data.is_ok() {
            let air = parse_json_metadata(data.unwrap());
            if air.is_ok() {
                for mii in air.unwrap().iter() {
                    if vec_arch.contains(&mii.arch.as_ref()) {
                        let src = &format!("{}{}", "docker://", mii.reference);
                        let dest = &format!("{}{}@{}", "file://", mii.namespace, mii.digest);
                        buf = buf + &format!("{}={}\n", src, dest);
                    }
                }
                fs_handler(
                    dir.clone() + "/mappings/additional-mapping.txt",
                    "write",
                    Some(buf),
                )?;
                log.mid(&format!(
                    "created additional images mapping file in folder {}",
                    dir.clone() + &"/mappings/",
                ));
            } else {
                let err = MirrorError::new(&format!(
                    "parsing additional images metadata file {}",
                    air.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        } else {
            let err = MirrorError::new(&format!(
                "reading additional images metadata file {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    } else {
        let map = remove_duplicates(dir.clone(), fslayers);
        let res = execute_batch(log, dir.clone(), verify_blobs, map).await;
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
