use mirror_error::MirrorError;
use mirror_utils::fs_handler;
use serde_derive::{Deserialize, Serialize};

/// config schema
#[derive(Serialize, Deserialize, Debug)]
pub struct ImageSetConfig {
    #[serde(rename = "kind")]
    pub kind: String,

    #[serde(rename = "apiVersion")]
    pub api_version: String,

    #[serde(rename = "mirror")]
    pub mirror: Mirror,

    #[serde(rename = "archiveSize")]
    pub archive_size: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Mirror {
    #[serde(rename = "release")]
    pub release: Option<Release>,

    #[serde(rename = "operators")]
    pub operators: Option<Vec<Operator>>,

    #[serde(rename = "additionalImages")]
    pub additional_images: Option<Vec<Image>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Image {
    #[serde(rename = "name")]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Operator {
    #[serde(rename = "catalog")]
    pub catalog: String,

    #[serde(rename = "packages")]
    pub packages: Option<Vec<Package>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Package {
    #[serde(rename = "name")]
    pub name: String,

    #[serde(rename = "bundles")]
    pub bundles: Option<Vec<Bundle>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Bundle {
    #[serde(rename = "name")]
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Release {
    #[serde(rename = "graph")]
    pub graph: Option<String>,

    #[serde(rename = "images")]
    pub images: Vec<Image>,
}

// read the 'image set config' file
pub async fn load_config(config_file: String) -> Result<String, MirrorError> {
    // Create a path to the desired file
    let data = fs_handler(config_file.clone(), "read", None).await?;
    Ok(data.clone())
}

// parse the 'image set config' file
pub fn parse_yaml_config(data: String) -> Result<ImageSetConfig, MirrorError> {
    // Parse the string of data into serde_json::ImageSetConfig.
    let res = serde_yaml::from_str(&data);
    if res.is_err() {
        return Err(MirrorError::new(&format!(
            "[parse_yaml_config] {}",
            res.err().unwrap().to_string().to_lowercase()
        )));
    }
    let root: ImageSetConfig = res.unwrap();
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
    fn load_config_pass() {
        let res = aw!(load_config(String::from("examples/imagesetconfig.yaml")));
        assert_eq!(res.is_ok(), true);
    }

    #[test]
    fn load_config_fail() {
        let res = aw!(load_config(String::from("nada.yaml")));
        assert_eq!(res.is_err(), true);
    }

    #[test]
    fn isc_parser_pass() {
        let data = aw!(load_config(String::from("examples/imagesetconfig.yaml")));
        let res = parse_yaml_config(data.unwrap());
        assert_eq!(res.is_ok(), true);
    }

    #[test]
    fn isc_parser_fail() {
        let data = "{ ".to_string();
        let res = parse_yaml_config(data);
        assert_eq!(res.is_err(), true);
    }
}
