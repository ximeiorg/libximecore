//! `xime-sync-server` 二进制薄入口：加载配置、校验密码、构建 runtime、调用库的 [`run`]。
//!
//! 服务器实现全部在 `lib.rs`（供 desktop 模式复用）。
//!
//! 用法：`xime-sync-server [--config <path>]`，`--config` 指定自定义配置文件
//! （供集成方把配置放在自己的配置目录，如 XimeChe 的 `~/.config/xime/xime-sync.toml`）。

use std::path::PathBuf;

use xime_sync_server::config::Settings;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 手动解析 `--config <path>`（不引入 clap，保持薄入口）
    let mut config_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => config_path = args.next().map(PathBuf::from),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let settings = match &config_path {
        Some(path) => Settings::load_from(Some(path))?,
        None => Settings::new()?,
    };

    // 敏感项校验：认证密码必须通过配置或环境变量提供
    if let Err(msg) = xime_sync_server::check_auth_password(&settings) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    xime_sync_server::init_tracing(&settings.server().log_level);

    // 按配置构建 tokio runtime：workers 可配，低端设备可设为 1
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    if let Some(workers) = settings.server().workers {
        builder.worker_threads(workers);
    }
    let rt = builder
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    rt.block_on(xime_sync_server::run(settings))
}
