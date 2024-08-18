use crate::api::schema::MirrorImageInfo;
use crate::error::handler::MirrorError;
use custom_logger::*;
use mirror_copy::*;
use std::fs;

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
    if image.contains("@") {
        let mut hld = image.split("@");
        let img = hld.nth(0).unwrap();
        let digest = hld.nth(0).unwrap();
        let vec_comp = img.split("/").collect::<Vec<&str>>();
        if vec_comp.len() == 3 {
            let ir = ImageReference {
                registry: vec_comp[0].to_string(),
                namespace: vec_comp[1].to_string(),
                name: vec_comp[2].to_string(),
                version: digest.to_string(),
            };
            log.debug(&format!("image reference {:#?}", ir.clone()));
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
        let mut empty_ir = ImageReference {
            registry: "".to_string(),
            namespace: "".to_string(),
            name: "".to_string(),
            version: "".to_string(),
        };
        if image.contains(":") {
            let mut hld = image.split(":");
            let img = hld.nth(0).unwrap();
            let tag = hld.nth(0).unwrap();
            let vec_comp = img.split("/").collect::<Vec<&str>>();
            if vec_comp.len() == 3 {
                let ir = ImageReference {
                    registry: vec_comp[0].to_string(),
                    namespace: vec_comp[1].to_string(),
                    name: vec_comp[2].to_string(),
                    version: tag.to_string(),
                };
                log.debug(&format!("image reference {:#?}", ir.clone()));
                return ir;
            } else {
                return empty_ir;
            }
        } else {
            empty_ir.registry = image;
            return empty_ir;
        }
    }
}

pub fn process_fb_image(
    dir: String,
    oci: String,
    reference: String,
    tag_digest: String,
    mirror_type: String,
) -> Result<MirrorImageInfo, MirrorError> {
    let index_json = format!("{}/manifest.json", &oci);
    let index_data = fs::read_to_string(index_json);
    let m = parse_json_manifest_operator(index_data.as_ref().unwrap().to_string());
    //let mut mii: MirrorImageInfo;
    if m.is_ok() {
        let mnfst = m.unwrap();
        for mn in mnfst.layers.unwrap().iter() {
            let digest = mn.digest.replace(":", "/");
            let blob = digest.split("sha256/").nth(1).unwrap();
            let to_path = format!("{}/blobs-store/{}", dir.clone(), &blob[0..2]);
            fs::create_dir_all(to_path.clone()).expect("should create blob directory");
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
        fs::copy(format!("{}/{}", &oci, blob), to).expect("should copy fb config blob");
        // finally write the manifest
        let manifest_file = format!(
            "{}/manifests/{}/{}:{}-all.json",
            dir.clone(),
            &mirror_type,
            oci,
            tag_digest
        );
        fs::write(manifest_file, index_data.unwrap())
            .expect("should write manifest to manifest cache");

        let mut mii = MirrorImageInfo {
            // TODO:should fix this to parse image
            reference: reference.clone(),
            name: oci.to_string(),
            arch: "all".to_string(),
            namespace: reference,
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
}

// parse the manifest json for operator indexes only
pub fn parse_json_metadata(
    data: String,
) -> Result<Vec<MirrorImageInfo>, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: Vec<MirrorImageInfo> = serde_json::from_str(&data)?;
    Ok(root)
}
