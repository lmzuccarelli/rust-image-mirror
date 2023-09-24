use crate::api::schema::*;
use crate::Path;
use std::fs::File;
use std::io::Read;

// read the 'image set config' file
pub fn load_config(dir: String) -> Result<String, Box<dyn std::error::Error>> {
    // Create a path to the desired file
    let path = Path::new(&dir);
    let display = path.display();

    // Open the path in read-only mode, returns `io::Result<File>`
    let mut file = match File::open(&path) {
        Err(why) => panic!("couldn't open {}: {}", display, why),
        Ok(file) => file,
    };

    // Read the file contents into a string, returns `io::Result<usize>`
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}

// parse the 'image set config' file
pub fn parse_yaml_config(data: String) -> Result<ImageSetConfig, serde_yaml::Error> {
    // Parse the string of data into serde_json::ImageSetConfig.
    let res = serde_yaml::from_str::<ImageSetConfig>(&data);
    res
}
