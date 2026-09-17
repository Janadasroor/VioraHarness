// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub(crate) async fn cmd_shim(port: u16) -> anyhow::Result<()> {
    vioraharness_server::shim::serve_shim(port).await
}
