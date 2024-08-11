use crate::api::schema::MirrorImageInfo;
use custom_logger::*;
use mirror_copy::ImageReference;

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
            let ir = ImageReference {
                registry: "".to_string(),
                namespace: "".to_string(),
                name: "".to_string(),
                version: "".to_string(),
            };
            return ir;
        }
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
