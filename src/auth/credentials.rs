use crate::api::schema::*;
use crate::log::logging::*;
use crate::Path;
use base64::{engine::general_purpose, Engine as _};
use std::env;
use std::fs::File;
use std::io::Read;
use std::str;

// read the credentials from set path (see podman credential reference)
pub fn get_credentials() -> Result<String, Box<dyn std::error::Error>> {
    // Create a path to the desired file
    // using $XDG_RUNTIME_DIR envar
    let u = match env::var_os("XDG_RUNTIME_DIR") {
        Some(v) => v.into_string().unwrap(),
        None => panic!("$XDG_RUNTIME_DIR is not set"),
    };
    let binding = &(u.to_owned() + "/containers/auth.json");
    let path = Path::new(binding);
    let display = path.display();

    // Open the path in read-only mode, returns `io::Result<File>`
    let mut file = match File::open(&binding) {
        Err(why) => panic!("couldn't open {}: {}", display, why),
        Ok(file) => file,
    };

    // Read the file contents into a string, returns `io::Result<usize>`
    let mut s = String::new();
    file.read_to_string(&mut s)?;
    Ok(s)
}

// parse the json credentials to a struct
pub fn parse_json_creds(log: &Logging, data: String) -> Result<String, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Root.
    let creds: Root = serde_json::from_str(&data)?;
    log.trace("using credentials for registry_redhat_io");
    Ok(creds.auths.registry_redhat_io.unwrap().auth)
}

// parse the json from the api call
pub fn parse_json_token(data: String) -> Result<String, Box<dyn std::error::Error>> {
    // Parse the string of data into serde_json::Token.
    let root: Token = serde_json::from_str(&data)?;
    Ok(root.access_token)
}

// async api call with basic auth
pub async fn get_auth_json(
    url: String,
    user: String,
    password: String,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let pwd: Option<String> = Some(password);
    let body = client
        .get(url)
        .basic_auth(user, pwd)
        .send()
        .await?
        .text()
        .await?;
    Ok(body)
}

// process all relative functions in this module to actaully get the token
pub async fn get_token(log: &Logging, name: String, url: String) -> String {
    let token_url = match name.as_str() {
        "registry.redhat.io" => "https://sso.redhat.com/auth/realms/rhcc/protocol/redhat-docker-v2/auth?service=docker-registry&client_id=curl&scope=repository:rhel:pull".to_string(),
        "test.registry.io" => url.clone(),
        &_ => "none".to_string(),
    };
    // get creds from $XDG_RUNTIME_DIR
    let creds = get_credentials().unwrap();
    // parse the json data
    let rhauth = parse_json_creds(&log, creds).unwrap();
    // decode to base64
    let bytes = general_purpose::STANDARD.decode(rhauth).unwrap();

    let s = match str::from_utf8(&bytes) {
        Ok(v) => v,
        Err(e) => panic!("ERROR: invalid UTF-8 sequence: {}", e),
    };
    // get user and password form json
    let user = s.split(":").nth(0).unwrap();
    let pwd = s.split(":").nth(1).unwrap();
    // call the realm url to get a token with the creds
    let res = get_auth_json(token_url, user.to_string(), pwd.to_string())
        .await
        .unwrap();
    // if all goes well we should have a valid token
    let token = parse_json_token(res).unwrap();
    token
}

#[cfg(test)]
mod tests {
    // this brings everything from parent's scope into this scope
    use super::*;
    use serial_test::serial;

    macro_rules! aw {
        ($e:expr) => {
            tokio_test::block_on($e)
        };
    }
    // first get the token to obtain highest level of coverage
    #[test]
    #[serial]
    fn test_get_token_pass() {
        env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let log = &Logging {
            log_level: Level::DEBUG,
        };
        let res = aw!(get_token(
            log,
            String::from("registry.redhat.io"),
            String::from("")
        ));
        assert!(res.to_string() != String::from(""));
    }

    #[test]
    #[serial]
    fn test_parse_json_creds_pass() {
        env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let log = &Logging {
            log_level: Level::DEBUG,
        };
        let data = get_credentials().unwrap();
        let res = parse_json_creds(log, data);
        assert!(res.is_ok());
    }

    #[test]
    #[serial]
    fn test_get_credentials_pass() {
        env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let res = get_credentials();
        assert!(res.is_ok());
    }

    #[test]
    #[serial]
    #[should_panic]
    fn test_get_credentials_nofile_fail() {
        env::set_var("XDG_RUNTIME_DIR", "/run/");
        let res = get_credentials();
        assert!(res.is_ok());
    }

    #[test]
    #[serial]
    #[should_panic]
    fn test_get_credentials_fail() {
        env::remove_var("XDG_RUNTIME_DIR");
        let res = get_credentials();
        assert!(res.is_err());
    }
}
