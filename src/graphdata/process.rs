use crate::podman::process::*;
use async_trait::async_trait;
use custom_logger::*;
use flate2::read::GzDecoder;
use mirror_error::MirrorError;
use mirror_utils::{fs_copy, fs_handler, fs_open_or_create};
use reqwest::{Client, StatusCode};
use std::fs;
use std::io::Cursor;
use std::path::Path;
use tar::Archive;

#[derive(Debug, Clone)]
pub struct ImplGraphDataInterface {}

#[allow(unused)]
#[async_trait]
pub trait GraphDataInterface {
    // used to interact with cincinnati api
    async fn get_graphdata(&self, url: String) -> Result<String, MirrorError>;
    async fn get_graph_tar_gz(&self, url: String, file_name: &str) -> Result<(), MirrorError>;
    async fn build_graph_image(
        &self,
        log: &Logging,
        dir: String,
        url: String,
    ) -> Result<(), MirrorError>;
    async fn build_image_cleanup(&self);
}

#[async_trait]
impl GraphDataInterface for ImplGraphDataInterface {
    async fn get_graphdata(&self, url: String) -> Result<String, MirrorError> {
        let client = Client::new();
        let res = client
            .get(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await;

        if res.is_err() || res.as_ref().unwrap().status() != StatusCode::OK {
            let err =
                MirrorError::new(&format!("[get_graphdata] status {}", res.unwrap().status()));
            return Err(err);
        }
        let body = res.unwrap().text().await;
        if body.is_err() {
            let err = MirrorError::new(&format!(
                "[get_graphdata] reading body data {}",
                body.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(body.unwrap())
    }

    async fn get_graph_tar_gz(&self, url: String, file_name: &str) -> Result<(), MirrorError> {
        let response = reqwest::get(url).await;
        if response.is_err() || response.as_ref().unwrap().status() != StatusCode::OK {
            let err = MirrorError::new(&format!(
                "[get_graph_tar_gz] status {}",
                response.as_ref().unwrap().status()
            ));
            return Err(err);
        }
        let mut file = fs_open_or_create(file_name.to_string(), true).await?;
        let data = response.unwrap().bytes().await;
        if data.is_err() {
            let err = MirrorError::new(&format!(
                "[get_graph_tar_gz] response data {}",
                data.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        let mut content = Cursor::new(data.unwrap());
        let res_copy = std::io::copy(&mut content, &mut file);
        if res_copy.is_err() {
            let err = MirrorError::new(&format!(
                "[get_graph_tar_gz] copying artifact {}",
                res_copy.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    async fn build_graph_image(
        &self,
        log: &Logging,
        dir: String,
        url: String,
    ) -> Result<(), MirrorError> {
        let tar_gz_file = format!("{}/artifacts/cincinnati-graph-data.tar.gz", dir);
        let exists = Path::new(&tar_gz_file).exists();
        if !exists {
            self.get_graph_tar_gz(url.clone(), &tar_gz_file).await?;
        }
        fs_handler("container".to_string(), "create_dir", None).await?;
        let file = fs_open_or_create(tar_gz_file, false).await?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);
        let res_archive = archive.unpack("container/");
        if res_archive.is_ok() {
            fs_copy(
                "hack/graph-data.containerfile".to_string(),
                "graph-data.containerfile".to_string(),
            )
            .await?;
            // build the graph image
            build(
                log,
                "openshift/graph-image:latest".to_string(),
                "graph-data.containerfile".to_string(),
            )
            .await?;
            save(
                log,
                "openshift/graph-image:latest".to_string(),
                "graph-image".to_string(),
            )
            .await?;
        } else {
            let err = MirrorError::new(&format!(
                "[build-graph-image] cincinnati-graph-data tar.gz {}",
                res_archive.err().unwrap().to_string().to_lowercase()
            ));
            return Err(err);
        }
        Ok(())
    }

    async fn build_image_cleanup(&self) {
        if Path::new("graph-data.containerfile").exists() {
            fs::remove_file("graph-data.containerfile").expect("delete containerfile");
        }
        if Path::new("container").exists() {
            fs::remove_dir_all("container").expect("should delete container directory");
        }
        if Path::new("graph-image").exists() {
            fs::remove_dir_all("graph-image").expect("delete graph-image directory");
        }
    }
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
    fn get_graphdata_pass() {
        fs::create_dir_all("test-artifacts/artifacts").expect("should create artifact directory");
        fs::copy(
            "test-artifacts/raw-tar-files/cincinnati-graph-data.tar.gz",
            "test-artifacts/artifacts/cincinnati-graph-data.tar.gz",
        )
        .expect("should copy tar.gz file");

        let _ = aw!(fs_handler(
            "test-artifacts/artifacts".to_string(),
            "create_dir",
            None
        ));
        // we set up a mock server for the auth-credentials
        let mut server = mockito::Server::new();
        let url = server.url();

        // Create a mock
        server
            .mock("GET", "/graph-data")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"all\": \"good\",
                }",
            )
            .create();

        server
            .mock("GET", "/graph-data-error")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"all\": \"fail\",
                }",
            )
            .create();

        server
            .mock("GET", "/graph-tar-gz")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"all\": \"good\",
                }",
            )
            .create();

        server
            .mock("GET", "/graph-tar-gz-error")
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(
                "{
                    \"all\": \"fail\",
                }",
            )
            .create();

        let log = &Logging {
            log_level: Level::INFO,
        };

        let g_impl = ImplGraphDataInterface {};
        log.hi(&format!("testing get_graphdata [should pass]"));
        let res = aw!(g_impl.get_graphdata(format!("{}/graph-data", url)));
        assert_eq!(res.is_ok(), true);

        log.hi(&format!("testing get_graphdata [should fail]"));
        let res = aw!(g_impl.get_graphdata(format!("{}/graph-data-error", url)));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_err(), true);

        // check graph tar.gz
        log.hi(&format!("testing get_graph_tar_gz [should pass]"));
        let res = aw!(g_impl.get_graph_tar_gz(format!("{}/graph-tar-gz", url), &"test.txt"));
        assert_eq!(res.is_ok(), true);

        log.hi(&format!("testing get_graph_tar_gz [should fail]"));
        let res = aw!(g_impl.get_graph_tar_gz(format!("{}/graph-tar-gz-error", url), &"test.txt"));
        if res.is_err() {
            log.error(&format!(
                "result -> {}",
                res.as_ref().err().unwrap().to_string().to_lowercase()
            ));
        }
        assert_eq!(res.is_err(), true);

        log.hi(&format!("testing build_graph_image [should pass]"));
        let res = aw!(g_impl.build_graph_image(log, "test-artifacts".to_string(), url));
        assert_eq!(res.is_ok(), true);
        let _ = aw!(g_impl.build_image_cleanup());
    }
}
