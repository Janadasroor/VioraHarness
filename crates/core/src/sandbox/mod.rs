// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub mod docker;
pub mod linux;
pub mod macos;

use tokio::process::Command;

pub fn apply_sandbox(cmd: &mut Command, workdir: &str) {
    if cfg!(target_os = "linux") {
        linux::wrap_command(cmd, workdir);
    } else if cfg!(target_os = "macos") {
        macos::wrap_command(cmd, workdir);
    } else {
        docker::wrap_command(cmd, workdir);
    }
}
