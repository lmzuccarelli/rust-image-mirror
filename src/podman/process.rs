use crate::error::handler::MirrorError;
use custom_logger::*;
use std::process::Command;

pub fn build(log: &Logging, image: String, container_file: String) -> Result<(), MirrorError> {
    let output = Command::new("podman")
        .arg("build")
        //.arg("-q")
        .arg("-t")
        .arg(&image)
        .arg("-f")
        .arg(&container_file)
        .output()
        .expect("failed to execute process");

    if output.status.success() {
        log.info("build image completed successfully");
    }
    log.debug(&format!(
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    ));
    log.debug(&format!(
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    ));

    assert!(output.status.success());
    Ok(())
}

pub fn save(log: &Logging, image: String, output_file: String) -> Result<(), MirrorError> {
    let output = Command::new("podman")
        .arg("save")
        .arg("--format")
        .arg("docker-dir")
        //.arg("-m")
        .arg("-o")
        .arg(output_file)
        .arg(image)
        .output()
        .expect("failed to execute process");

    if output.status.success() {
        log.info("save image (v2d2) to disk completed successfully");
    }
    log.debug(&format!(
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    ));
    log.debug(&format!(
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    ));

    assert!(output.status.success());
    Ok(())
}
