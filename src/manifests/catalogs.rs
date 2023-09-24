use crate::api::schema::*;
use crate::log::logging::*;
use std::error::Error;
use std::fs;
use std::fs::File;
use std::io::Read;

// read_operator_catalog - simple function tha treads the specific catalog.json file
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

// find a specifc directory in the untar layers
pub async fn find_dir(log: &Logging, dir: String, name: String) -> String {
    // env::set_current_dir("../../../../").expect("could not set current directory");
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
