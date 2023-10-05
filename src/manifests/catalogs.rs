use crate::api::schema::*;
use crate::log::logging::*;

use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::Read;

// read_operator_catalog - simple function that reads the specific catalog.json file
// and unmarshals it to DeclarativeConfig struct
pub fn read_operator_catalog(path: String) -> Result<serde_json::Value, Box<dyn Error>> {
    let catalog = path + &"/catalog.json".to_owned();
    // Open the path in read-only mode, returns `io::Result<File>`
    let mut file = match File::open(&catalog) {
        Err(why) => panic!("couldn't open {}: {}", catalog, why),
        Ok(file) => file,
    };

    // Read the file contents into a string, returns `io::Result<usize>`
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    let res = s.replace(" ", "");
    // update to allow for well formatted json so that it can be processed
    let updated_json =
        "{ \"overview\": [".to_string() + &res.replace("}\n{", "},{") + &"]}".to_string();
    // Parse the string of data into serde_json::Vec<DeclarativeConfig>
    let root = match serde_json::from_str::<Catalog>(&updated_json) {
        Ok(val) => val,
        Err(error) => panic!("error {}", error),
    };
    Ok(root.overview)
}

// find a specific directory in the untar layers
pub async fn find_dir(log: &Logging, dir: String, name: String) -> String {
    let paths = fs::read_dir(&dir);
    // for both release & operator image indexes
    // we know the layer we are looking for is only 1 level
    // down from the parent
    match paths {
        Ok(res_paths) => {
            for path in res_paths {
                let entry = path.expect("could not resolve path entry");
                let file = entry.path();
                // go down one more level
                let sub_paths = fs::read_dir(file).unwrap();
                for sub_path in sub_paths {
                    let sub_entry = sub_path.expect("could not resolve sub path entry");
                    let sub_name = sub_entry.path();
                    let str_dir = sub_name.into_os_string().into_string().unwrap();
                    if str_dir.contains(&name) {
                        return str_dir;
                    }
                }
            }
        }
        Err(error) => {
            let msg = format!("{} ", error);
            log.warn(&msg);
        }
    }
    return "".to_string();
}

// parse the manifest json for operator indexes only
pub fn parse_json_manifest(data: String) -> Result<ManifestSchema, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::ManifestSchema.
    let root: ManifestSchema = serde_json::from_str(&data)?;
    Ok(root)
}

// parse the manifest json for operators
pub fn parse_json_manifest_operator(data: String) -> Result<Manifest, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Manifest.
    let root: Manifest = serde_json::from_str(&data)?;
    Ok(root)
}

// parse the manifest list json
pub fn parse_json_manifestlist(data: String) -> Result<ManifestList, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::ManifestList.
    let root: ManifestList = serde_json::from_str(&data)?;
    Ok(root)
}

// contruct the manifest url
pub fn get_image_manifest_url(image_ref: ImageReference) -> String {
    // return a string in the form of (example below)
    // "https://registry.redhat.io/v2/redhat/certified-operator-index/manifests/v4.12";
    let mut url = String::from("https://");
    url.push_str(&image_ref.registry);
    url.push_str(&"/v2/");
    url.push_str(&image_ref.namespace);
    url.push_str(&"/");
    url.push_str(&image_ref.name);
    url.push_str(&"/");
    url.push_str(&"manifests/");
    url.push_str(&image_ref.version);
    url
}

// contruct a manifest url from a string
pub fn get_manifest_url(url: String) -> String {
    let mut parts = url.split("/");
    let mut url = String::from("https://");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/v2/");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/");
    let i = parts.nth(0).unwrap();
    let mut sha = i.split("@");
    url.push_str(&sha.nth(0).unwrap());
    url.push_str(&"/");
    url.push_str(&"manifests/");
    url.push_str(&sha.nth(0).unwrap());
    url
}

// contruct a manifest url from a string by digest
pub fn get_manifest_url_by_digest(url: String, digest: String) -> String {
    let mut parts = url.split("/");
    let mut url = String::from("https://");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/v2/");
    url.push_str(&parts.nth(0).unwrap());
    url.push_str(&"/");
    let i = parts.nth(0).unwrap();
    let mut sha = i.split("@");
    url.push_str(&sha.nth(0).unwrap());
    url.push_str(&"/");
    url.push_str(&"manifests/");
    url.push_str(&digest);
    url
}

// utility functions - get_manifest_json
pub fn get_manifest_json_file(name: String, version: String) -> String {
    let mut file = String::from("working-dir/");
    file.push_str(&name);
    file.push_str(&"/");
    file.push_str(&version);
    file.push_str(&"/");
    file.push_str(&"manifest.json");
    file
}
