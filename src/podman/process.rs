use custom_logger::*;
use mirror_error::MirrorError;
use std::process::Command;

pub async fn build(
    log: &Logging,
    image: String,
    container_file: String,
) -> Result<(), MirrorError> {
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
        log.ex("[build] image completed successfully");
    }
    log.debug(&format!(
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    ));
    log.debug(&format!(
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    ));
    if !output.status.success() {
        return Err(MirrorError::new(&format!(
            "[build] {:?}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    assert!(output.status.success());
    Ok(())
}

pub async fn save(log: &Logging, image: String, output_file: String) -> Result<(), MirrorError> {
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
        log.ex("[save] saving image to disk completed successfully");
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
