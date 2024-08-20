use crate::error::handler::MirrorError;
use serde_derive::{Deserialize, Serialize};
use std::fs;

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

#[derive(Serialize, Deserialize, Debug)]
pub struct Release {
    #[serde(rename = "graph")]
    pub graph: Option<String>,

    #[serde(rename = "images")]
    pub images: Vec<Image>,
}

// read the 'image set config' file
pub fn load_config(config_file: String) -> Result<String, MirrorError> {
    // Create a path to the desired file
    let data = fs::read_to_string(config_file.clone());
    if data.is_ok() {
        Ok(data.unwrap())
    } else {
        let err = MirrorError::new(&format!("could not read config file {} ", config_file));
        Err(err)
    }
}

// parse the 'image set config' file
pub fn parse_yaml_config(data: String) -> Result<ImageSetConfig, serde_yaml::Error> {
    // Parse the string of data into serde_json::ImageSetConfig.
    let res = serde_yaml::from_str::<ImageSetConfig>(&data);
    res
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;

    #[test]
    fn test_load_config_pass() {
        let res = load_config(String::from("./imagesetconfig.yaml"));
        assert!(res.is_ok());
    }

    #[test]
    #[should_panic]
    fn test_load_config_fail() {
        let res = load_config(String::from("./nada.yaml"));
        assert!(res.is_err());
    }

    // finally test that the parser is working correctly
    #[test]
    fn test_isc_parser() {
        let data = load_config(String::from("./imagesetconfig.yaml"));
        let res = parse_yaml_config(data.unwrap().to_string());
        assert!(res.is_ok());
    }
}
