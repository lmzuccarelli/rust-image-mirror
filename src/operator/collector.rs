use crate::api::schema::*;
use crate::log::logging::*;
use crate::manifests::catalogs::*;
//use semver::{BuildMetadata, Prerelease, Version};
//use std::cmp::*;
//use std::fs;

// collect all operator images
pub async fn mirror_to_disk(
    log: &Logging,
    dir: String,
    packages: Vec<String>,
) -> Vec<RelatedImage> {
    let mut bundle_name = String::from("");
    let mut related_images = Vec::new();

    for pkg in packages {
        let dc_json = read_operator_catalog(dir.to_string() + &"/".to_string() + &pkg)
            .unwrap()
            .clone();
        let dc: Vec<DeclarativeConfig> = serde_json::from_value(dc_json.clone()).unwrap();

        log.lo(&format!("default channel {:?} ", dc[0].default_channel));

        for obj in dc.iter() {
            if obj.schema == "olm.bundle" {
                if bundle_name == obj.name {
                    log.trace(&format!("bundle {:#?} {}", obj.related_images, obj.name));
                    related_images = obj.related_images.clone().unwrap();
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
    related_images
}
