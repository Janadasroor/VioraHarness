pub mod app;
pub mod theme;
pub mod widgets;

pub async fn run(model: Option<String>) -> anyhow::Result<String> {
    let m = model
        .or_else(|| std::env::var("VIORAHARNESS_MODEL").ok())
        .unwrap_or_default();
    let app = app::App::new(m);
    app.run().await
}
