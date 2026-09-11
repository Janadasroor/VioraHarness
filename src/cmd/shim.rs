pub(crate) async fn cmd_shim(port: u16) -> anyhow::Result<()> {
    vioraharness_server::shim::serve_shim(port).await
}
