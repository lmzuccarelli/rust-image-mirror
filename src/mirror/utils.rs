use crate::api::schema::MirrorImageInfo;
use custom_logger::*;
use mirror_catalog_index::ManifestSchema;
use mirror_copy::{FsLayer, ImageReference, Manifest, ManifestList};
use mirror_error::MirrorError;
use sha256::digest;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

// used to drive a spinner
#[allow(unused)]
pub mod keepalive {
    use std::sync::{Arc, Weak};

    pub struct Sender(Arc<()>);

    #[derive(Clone)]
    pub struct Receiver(Weak<()>);

    pub fn channel() -> (Sender, Receiver) {
        let arc = Arc::new(());
        let weak = Arc::downgrade(&arc);
        (Sender(arc), Receiver(weak))
    }

    impl Receiver {
        pub fn is_alive(&self) -> bool {
            Weak::strong_count(&self.0) > 0
        }
    }
}

pub fn parse_image(log: &Logging, image: String) -> ImageReference {
    // check if we have digest
    log.debug(&format!("[parse_image] parsing image {}", image.clone()));
    if image.contains("@") {
        let mut hld = image.split("@");
        let img = hld.nth(0).unwrap();
        // strip registry out
        let digest = hld.nth(0).unwrap();
        let vec_comp = img.split("/").collect::<Vec<&str>>();
        if vec_comp.len() == 3 {
            let ir = ImageReference {
                registry: vec_comp[0].to_string(),
                namespace: vec_comp[1].to_string(),
                name: vec_comp[2].to_string(),
                version: digest.to_string(),
            };
            log.debug(&format!("image input {}", image.clone()));
            log.debug(&format!("image digest reference {:#?}", ir.clone()));
            return ir;
        } else {
            let ir = ImageReference {
                registry: "".to_string(),
                namespace: "".to_string(),
                name: "".to_string(),
                version: "".to_string(),
            };
            return ir;
        }
    } else {
        let empty_ir = ImageReference {
            registry: "".to_string(),
            namespace: "".to_string(),
            name: "".to_string(),
            version: "".to_string(),
        };
        let components = image.split("/").collect::<Vec<&str>>();
        if components.len() >= 3 {
            if components[2].to_string().contains(":") {
                let name = components[2].split(":").nth(0).unwrap();
                let tag = components[2].split(":").nth(1).unwrap();
                let ir = ImageReference {
                    registry: components[0].to_string(),
                    namespace: components[1].to_string(),
                    name: name.to_string(),
                    version: tag.to_string(),
                };
                log.debug(&format!("image input {}", image.clone()));
                log.debug(&format!("image tag reference {:#?}", ir.clone()));
                return ir;
            } else {
                return empty_ir;
            }
        } else {
            return empty_ir;
        }
    }
}

pub async fn process_fb_image(
    dir: String,
    oci: String,
    reference: String,
    tag_digest: String,
    mirror_type: String,
) -> Result<MirrorImageInfo, MirrorError> {
    let index_json = format!("{}/manifest.json", &oci);
    let res_data = fs::read_to_string(index_json);
    if res_data.is_ok() {
        let m = parse_json_manifest_operator(res_data.as_ref().unwrap().to_string());
        if m.is_ok() {
            let mnfst = m.unwrap();
            for mn in mnfst.layers.unwrap().iter() {
                let digest = mn.digest.replace(":", "/");
                let blob = digest.split("sha256/").nth(1).unwrap();
                let to_path = format!("{}/blobs-store/{}", dir.clone(), &blob[0..2]);
                fs_handler(to_path.clone(), "create_dir", None).await?;
                fs::copy(
                    format!("{}/{}", &oci, blob),
                    format!("{}/{}", to_path, blob),
                )
                .expect("should copy blob");
            }
            // copy the config
            let cfg = mnfst.config;
            let blob = cfg.as_ref().unwrap().digest.split(":").nth(1).unwrap();
            let to = format!("{}/blobs-store/{}/{}", dir.clone(), &blob[0..2], blob);
            let to_path = format!("{}/blobs-store/{}", dir.clone(), &blob[0..2]);
            fs_handler(to_path.clone(), "create_dir", None).await?;
            fs::copy(format!("{}/{}", &oci, blob), to).expect("should copy fb config blob");
            // finally write the manifest
            let manifest_file = format!(
                "{}/manifests/{}/{}:{}-amd64.json",
                dir.clone(),
                &mirror_type,
                oci,
                tag_digest
            );
            fs_handler(manifest_file, "write", Some(res_data.unwrap())).await?;

            let mut mii = MirrorImageInfo {
                // TODO:should fix this to parse image
                reference: oci.to_string(),
                name: oci.to_string(),
                arch: "amd64".to_string(),
                namespace: reference + &"/" + &oci,
                digest: "".to_string(),
                manifest_type: "manifest".to_string(),
                tag: Some(tag_digest.clone()),
                created: "".to_string(),
                mirror_type: mirror_type.to_string(),
                bundle: None,
            };

            if tag_digest.contains("sha256:") {
                mii.digest = blob.to_string();
                mii.tag = None;
            }
            Ok(mii)
        } else {
            let err = MirrorError::new(&format!(
                "parsing fb manifest {}",
                m.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
    } else {
        let err = MirrorError::new(&format!(
            "reading fb manifest {}",
            res_data.err().unwrap().to_string().to_lowercase()
        ));
        return Err(err);
    }
}

pub fn remove_duplicates(
    dir: String,
    map_in: HashMap<String, Vec<FsLayer>>,
) -> HashMap<String, Vec<FsLayer>> {
    let mut map: HashMap<String, Vec<FsLayer>> = HashMap::new();
    let mut vec_layer: Vec<FsLayer> = Vec::new();
    // remove duplicates
    for (k, v) in map_in.iter() {
        for layer in v.iter() {
            // scrub out duplicates
            let truncated_image = layer.blob_sum.split(":").nth(1).unwrap();
            let inner_blobs_file = format!(
                "{}/{}/{}/{}",
                dir.clone(),
                "blobs-store",
                &truncated_image[0..2],
                truncated_image
            );
            let mut exists = Path::new(&inner_blobs_file).exists();
            if exists {
                let metadata = fs::metadata(&inner_blobs_file).unwrap();
                if layer.size.is_some() {
                    if metadata.len() != layer.size.unwrap() as u64 {
                        exists = false;
                    }
                } else {
                    exists = false;
                }
            }
            if !exists {
                vec_layer.insert(0, layer.clone());
            }
        }
        map.insert(k.to_string(), vec_layer.clone());
    }
    map
}

// parse the manifest json
pub fn parse_json_metadata(data: String) -> Result<Vec<MirrorImageInfo>, MirrorError> {
    // Parse the string of data into serde_json::Manifest.
    let res = serde_json::from_str(&data);
    if res.is_err() {
        return Err(MirrorError::new(&format!(
            "[parse_json_metadata] {}",
            res.err().unwrap().to_string().to_lowercase()
        )));
    }
    let root: Vec<MirrorImageInfo> = res.unwrap();
    Ok(root)
}

pub async fn fs_handler(
    dir_file: String,
    mode: &str,
    data: Option<String>,
) -> Result<String, MirrorError> {
    match mode {
        "create_dir" => {
            let res = fs::create_dir_all(&dir_file);
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "creating directory {} {}",
                    dir_file,
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }
        "remove_file" => {
            let res = fs::remove_file(&dir_file);
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "deleting file {} {}",
                    dir_file,
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }
        "remove_dir" => {
            let res = fs::remove_dir_all(&dir_file);
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "deleting directory {} {}",
                    dir_file,
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }
        "write" => {
            let res = fs::write(&dir_file, data.unwrap());
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "writing file {} {}",
                    dir_file,
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
        }
        "read" => {
            let res = fs::read_to_string(&dir_file);
            if res.is_err() {
                let err = MirrorError::new(&format!(
                    "reading file {} {}",
                    dir_file,
                    res.err().unwrap().to_string().to_lowercase()
                ));
                return Err(err);
            }
            return Ok(res.unwrap());
        }
        _ => {
            let err = MirrorError::new(&format!("mode {} not supported", dir_file,));
            return Err(err);
        }
    }
    Ok("ok".to_string())
}

// verify_file - function to check size and sha256 hash of contents
pub async fn verify_file(
    log: &Logging,
    dir: String,
    blob_sum: String,
    blob_size: u64,
    data: Vec<u8>,
) -> Result<(), MirrorError> {
    let f = &format!("{}/{}", dir, blob_sum);
    let res = fs::metadata(&f);
    if res.is_ok() {
        log.info(&format!("verifying blob  {}", &blob_sum));
        if res.unwrap().size() != blob_size {
            let err = MirrorError::new(&format!(
                "sha256 file size don't match {}",
                blob_size.clone()
            ));
            return Err(err);
        }
        let hash = digest(&data);
        if hash != blob_sum {
            let err = MirrorError::new(&format!(
                "sha256 hash contents don't match {}",
                blob_sum.clone()
            ));
            return Err(err);
        }
    } else {
        let err = MirrorError::new(&format!("sha256 hash metadata file {}", f));
        return Err(err);
    }
    Ok(())
}

// parse the manifest json
pub fn parse_json_manifest(data: String) -> Result<ManifestSchema, MirrorError> {
    // Parse the string of data into serde_json::ManifestSchema.
    let res = serde_json::from_str(&data);
    if res.is_err() {
        let err = MirrorError::new(&format!(
            "[parse_json_manifest] {}",
            res.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let root: ManifestSchema = res.unwrap();
    Ok(root)
}

// parse the manifestlist json
pub fn parse_json_manifestlist(data: String) -> Result<ManifestList, MirrorError> {
    // Parse the string of data into serde_json::ManifestList.
    let res = serde_json::from_str(&data);
    if res.is_err() {
        let err = MirrorError::new(&format!(
            "[parse_json_manifestlist] manifestlist {}",
            res.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let root: ManifestList = res.unwrap();
    Ok(root)
}

// parse the oci manifest json
pub fn parse_json_manifest_operator(data: String) -> Result<Manifest, MirrorError> {
    // Parse the string of data into serde_json::Manifest.
    let res = serde_json::from_str(&data);
    if res.is_err() {
        let err = MirrorError::new(&format!(
            "[parse_json_manifest_operator] {}",
            res.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let root: Manifest = res.unwrap();
    Ok(root)
}

pub fn read_and_parse_manifest(file: String) -> Result<ManifestSchema, MirrorError> {
    let data = fs::read_to_string(file.clone());
    if data.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_manifest] reading manifest data {} {}",
            file,
            data.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let manifest = parse_json_manifest(data.unwrap());
    if manifest.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_manifest] parsing manifest data {} {}",
            file,
            manifest.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    Ok(manifest.unwrap())
}

pub fn read_and_parse_oci_manifest(file: String) -> Result<Manifest, MirrorError> {
    let data = fs::read_to_string(file.clone());
    if data.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifest] reading oci manifest data {} {}",
            file,
            data.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let manifest = parse_json_manifest_operator(data.unwrap());
    if manifest.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifest] parsing oci manifest data {} {}",
            file,
            manifest.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    Ok(manifest.unwrap())
}

pub fn read_and_parse_oci_manifestlist(file: String) -> Result<ManifestList, MirrorError> {
    let data = fs::read_to_string(file.clone());
    if data.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifestlist] reading oci manifest data {} {}",
            file,
            data.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let manifest = parse_json_manifestlist(data.unwrap());
    if manifest.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifestlist] parsing oci manifest data {} {}",
            file,
            manifest.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    Ok(manifest.unwrap())
}

pub fn read_and_parse_metadata(file: String) -> Result<Vec<MirrorImageInfo>, MirrorError> {
    let data = fs::read_to_string(file.clone());
    if data.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifestlist] reading mirror-metadata {} {}",
            file,
            data.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    let md = parse_json_metadata(data.unwrap());
    if md.is_err() {
        let err = MirrorError::new(&format!(
            "[read_and_parse_oci_manifestlist] parsing mirror-metadata {} {}",
            file,
            md.err().unwrap().to_string().to_lowercase(),
        ));
        return Err(err);
    }
    Ok(md.unwrap())
}

pub async fn process_and_update_manifest(
    log: &Logging,
    manifest: String,
    file: String,
    file_override: HashMap<String, String>,
) -> Result<Option<String>, MirrorError> {
    log.debug(&format!(
        "[pocess_and_update_manifest] file {} ",
        file.clone()
    ));
    let mut changed = false;
    let res_file = file_override.get(&file.clone());
    if res_file.is_some() {
        log.debug(&format!("using override file {}", res_file.unwrap()));
        return Ok(Some(res_file.unwrap().to_string()));
    }
    let exists = Path::new(&file.clone()).exists();
    if exists {
        let manifest_on_disk = fs::read_to_string(file.clone());
        if manifest_on_disk.is_ok() {
            if manifest_on_disk.unwrap() != manifest {
                changed = true;
            }
        } else {
            changed = true;
        }
    }
    if !exists || changed {
        fs_handler(file.clone(), "write", Some(manifest.clone().to_string())).await?;
        return Ok(Some(file.clone()));
    }
    Ok(None)
}
