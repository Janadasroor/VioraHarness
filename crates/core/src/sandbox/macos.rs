pub fn is_seatbelt_available() -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("sandbox-exec")
            .arg("-h")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

pub fn wrap_command(_base: &mut tokio::process::Command, _workdir: &str) {}
