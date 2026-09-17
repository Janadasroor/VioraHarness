// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub fn is_docker_available() -> bool {
    std::process::Command::new("docker")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn wrap_command(_base: &mut tokio::process::Command, _workdir: &str) {
    // TODO: wrap with `docker run --rm -v workdir:/work -w /work ...`
}
