use crate::podman::process::*;
use custom_logger::*;
use mirror_catalog_index::find_dir;
use mirror_error::MirrorError;
use mirror_utils::{
    fs_copy, fs_handler, fs_open_or_create, parse_image, process_fb_image, MirrorImageInfo,
};
use serde_derive::{Deserialize, Serialize};
use std::io::Read;

#[derive(Serialize, Deserialize)]
pub struct CatalogHeader {
    #[serde(rename = "schema")]
    pub schema: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "defaultChannel")]
    pub default_channel: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CatalogCopyInfo {
    #[serde(rename = "catalog")]
    pub catalog: String,
    #[serde(rename = "package")]
    pub package: String,
    #[serde(rename = "channel")]
    pub channel: String,
}

#[derive(Debug, Clone)]
pub struct ImplCatalogBuildInterface {}

pub trait CatalogBuildInterface {
    async fn build_catalog(
        &self,
        log: &Logging,
        base_dir: String,
        catalogs: Vec<CatalogCopyInfo>,
    ) -> Result<Vec<MirrorImageInfo>, MirrorError>;
}

impl CatalogBuildInterface for ImplCatalogBuildInterface {
    async fn build_catalog(
        &self,
        log: &Logging,
        base_dir: String,
        catalogs: Vec<CatalogCopyInfo>,
    ) -> Result<Vec<MirrorImageInfo>, MirrorError> {
        let mut vec_mii: Vec<MirrorImageInfo> = Vec::new();
        let mut vec_catalogs: Vec<String> = Vec::new();
        for catalog in catalogs.iter() {
            if !vec_catalogs.contains(&catalog.catalog) {
                vec_catalogs.insert(0, catalog.catalog.clone());
            }
            create_filtered_config(log, base_dir.clone(), catalog.clone()).await?;
        }

        // loop throught each catalog again
        // this seems a waste to loop over once more
        // but its to ensure we do not duplicate the rebuild
        // catalog process (i.e multiple packages from same catalog)
        let current_dir = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        for catalog in vec_catalogs.clone().iter() {
            let ir = parse_image(log, catalog.clone());
            let catalog_dir = format!("{}/tmp/{}/{}", base_dir.clone(), ir.name, ir.version);
            let _ = std::env::set_current_dir(catalog_dir);
            fs_copy(
                format!("{}/hack/rebuild-catalog.containerfile", current_dir),
                "rebuild-catalog.containerfile".to_string(),
            )
            .await?;

            let data =
                fs_handler("rebuild-catalog.containerfile".to_string(), "read", None).await?;
            let updated = data.clone().replacen("{{ catalog }}", &catalog.clone(), 2);
            fs_handler(
                "rebuild-catalog.containerfile".to_string(),
                "write",
                Some(updated.to_string()),
            )
            .await?;

            build(
                log,
                catalog.clone(),
                "rebuild-catalog.containerfile".to_string(),
            )
            .await?;
            save(log, catalog.clone(), ir.name.clone()).await?;
            // finally save blobs and manifests to folders and data structure
            let res_fbi = process_fb_image(
                format!("{}/{}", current_dir.clone(), base_dir.clone()),
                ir.name.clone(),
                ir.namespace,
                ir.version,
                "operator".to_string(),
            )
            .await?;
            vec_mii.insert(0, res_fbi.clone());
        }
        let _ = std::env::set_current_dir(current_dir.clone());
        cleanup(base_dir.clone() + &"/tmp").await?;
        Ok(vec_mii)
    }
}

async fn create_filtered_config(
    log: &Logging,
    base_dir: String,
    catalog: CatalogCopyInfo,
) -> Result<(), MirrorError> {
    // create a temp working folder
    let ir = parse_image(log, catalog.catalog.clone());
    let working_dir_cache = format!(
        "{}/{}/{}/{}/cache",
        base_dir.clone(),
        ir.name.clone(),
        ir.version.clone(),
        "amd64".to_string(),
    );
    let config_dir = find_dir(log, working_dir_cache.clone(), "configs".to_string()).await;
    log.debug(&format!(
        "[create_filtered_config] operator config directory {} {}",
        working_dir_cache, config_dir
    ));
    let to_configs = format!(
        "{}/tmp/{}/{}/{}/{}",
        base_dir.clone(),
        ir.name.clone(),
        ir.version.clone(),
        "configs",
        catalog.package
    );
    fs_handler(to_configs.clone(), "create_dir", None).await?;
    let file_name = format!("{}/{}/catalog.json", config_dir, catalog.package);
    // Open the path in read-only mode, returns `Result()`
    let mut f = fs_open_or_create(file_name.to_string(), false).await?;

    // do some funky splicing
    // Read the file contents into a string, returns `io::Result<usize>`
    let mut s = String::new();
    let res = f.read_to_string(&mut s);
    if res.is_ok() {
        let chunks = s.split("}\n{");
        // TODO: consider other edge case
        let l = chunks.clone().count();
        for (pos, item) in chunks.clone().enumerate() {
            // first chunk
            if pos == 0 {
                let update = item.to_string() + "}";
                let header = parse_json_header(update.clone()).unwrap();
                let file_name = format!(
                    "{}/{}-{}.json",
                    to_configs.clone(),
                    header.name,
                    header.schema
                );
                fs_handler(file_name, "write", Some(update)).await?;
            }
            // last chunk
            if pos == l - 1 {
                let update = "{".to_string() + item;
                let header = parse_json_header(update.clone()).unwrap();
                let file_name = format!("{}/{}-{}.json", to_configs, header.name, header.schema);
                fs_handler(file_name, "write", Some(update)).await?;
            }
            // everything in between
            if pos > 0 && pos <= l - 2 {
                let update = "{".to_string() + item + "}";
                let header = parse_json_header(update.clone()).unwrap();
                let file_name = format!("{}/{}-{}.json", to_configs, header.name, header.schema);
                fs_handler(file_name, "write", Some(update)).await?;
            }
        }
    }

    // we now have all the relevant catalog files
    // take the package, channel and bundle
    // check if the channel != defaultChannel
    let package = format!("{}/{}-{}.json", to_configs, catalog.package, "olm.package");
    let pkg_data = fs_handler(package.clone(), "read", None).await?;
    let pkg_json = parse_json_header(pkg_data.clone()).unwrap();
    if pkg_json.default_channel.is_some() {
        let default_channel = pkg_json.default_channel.unwrap();
        if catalog.channel != default_channel {
            let updated_pkg_data = pkg_data.replace(&default_channel, &catalog.channel);
            fs_handler(package.clone(), "write", Some(updated_pkg_data)).await?;
        }
    }
    Ok(())
}

async fn cleanup(dir: String) -> Result<(), MirrorError> {
    // finally cleanup
    fs_handler(dir.clone(), "remove_dir", None).await?;
    Ok(())
}

// parse the manifest json for operator indexes only
pub fn parse_json_header(data: String) -> Result<CatalogHeader, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: CatalogHeader = serde_json::from_str(&data)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }

    #[test]

    fn build_catalog_pass() {
        let _ = aw!(fs_handler(
            "test-artifacts/manifests/operator".to_string(),
            "create_dir",
            None
        ));
        let log = &Logging {
            log_level: Level::TRACE,
        };
        let g_impl = ImplCatalogBuildInterface {};
        let cci = CatalogCopyInfo {
            catalog: "registry.redhat.io/redhat/redhat-operator-index:v4.15".to_string(),
            package: "windows-machine-config-operator".to_string(),
            channel: "stable".to_string(),
        };
        let vec_c = vec![cci];
        let res = aw!(g_impl.build_catalog(log, "test-artifacts".to_string(), vec_c.clone()));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase(),
            ));
        }
        assert_eq!(res.is_ok(), true);
    }
}
