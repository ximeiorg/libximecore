//! 语音识别守护进程。
//!
//! 只做三件事：读参数 → 建服务 → 起 socket。平台托管（launchd / Windows 服务 /
//! systemd user unit）归宿主，不写在这里。
//!
//! ```text
//! xime-speechd --socket ~/Library/Application\ Support/XimeYi/speech.sock \
//!              --models ~/Library/Application\ Support/XimeYi/models/asr \
//!              [--device cpu|metal|cuda] [--dtype f32|f16] [--idle-secs 300]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use xime_speech_service::{DType, DeviceKind, ServiceConfig, SpeechService};

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{}", Args::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("参数错误：{error}\n\n{}", Args::USAGE);
            return ExitCode::FAILURE;
        }
    };

    let provider = args.device.as_str().to_uppercase();
    let mut config = ServiceConfig::new(&args.models);
    config.device = args.device;
    config.dtype = args.dtype;
    config.idle_unload = Duration::from_secs(args.idle_secs);
    config.provider = provider;

    let service = Arc::new(SpeechService::new(config));
    eprintln!(
        "[xime-speechd] socket={} models={}",
        args.socket,
        args.models.display()
    );
    match xime_speech_service::transport::serve(&args.socket, service) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[xime-speechd] {error}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    /// Unix socket 路径；Windows 上是命名管道（可用 `xime-speech` 简写）。
    socket: String,
    models: PathBuf,
    device: DeviceKind,
    dtype: DType,
    idle_secs: u64,
}

impl Args {
    const USAGE: &'static str = "\
用法: xime-speechd --socket <路径> --models <模型根目录> [选项]

选项:
  --device cpu|metal|cuda   推理后端（默认 cpu）
  --dtype f32|f16|bf16      权重精度（默认 f32；Metal 建议 f16）
  --idle-secs <秒>           空闲多久卸载模型（默认 300）
  --help                     显示这段说明

环境变量: XIME_SPEECH_SOCKET / XIME_SPEECH_MODELS 作为参数缺省值。";

    /// `Ok(None)` = 用户要 --help。
    fn parse(args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut socket = std::env::var("XIME_SPEECH_SOCKET").ok();
        let mut models = std::env::var_os("XIME_SPEECH_MODELS").map(PathBuf::from);
        let mut device = "cpu".to_string();
        let mut dtype = "f32".to_string();
        let mut idle_secs = 300u64;

        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--help" | "-h" => return Ok(None),
                "--socket" => socket = Some(next_value(&mut args, &flag)?),
                "--models" => models = Some(next_value(&mut args, &flag)?.into()),
                "--device" => device = next_value(&mut args, &flag)?,
                "--dtype" => dtype = next_value(&mut args, &flag)?,
                "--idle-secs" => {
                    idle_secs = next_value(&mut args, &flag)?
                        .parse()
                        .map_err(|_| "--idle-secs 需要整数".to_string())?;
                }
                other => return Err(format!("未知参数：{other}")),
            }
        }

        Ok(Some(Self {
            socket: socket.ok_or("缺少 --socket（或 XIME_SPEECH_SOCKET）")?,
            models: models.ok_or("缺少 --models（或 XIME_SPEECH_MODELS）")?,
            device: parse_device(&device)?,
            dtype: parse_dtype(&dtype)?,
            idle_secs,
        }))
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} 缺少取值"))
}

fn parse_device(value: &str) -> Result<DeviceKind, String> {
    match value {
        "cpu" => Ok(DeviceKind::Cpu),
        #[cfg(all(target_os = "macos", feature = "metal"))]
        "metal" => Ok(DeviceKind::Metal),
        #[cfg(feature = "cuda")]
        "cuda" => Ok(DeviceKind::Cuda),
        other => Err(format!(
            "未知后端：{other}（可用：cpu{}）",
            if cfg!(feature = "cuda") {
                "、cuda"
            } else {
                ""
            }
        )),
    }
}

fn parse_dtype(value: &str) -> Result<DType, String> {
    match value {
        "f32" => Ok(DType::F32),
        "f16" => Ok(DType::F16),
        "bf16" => Ok(DType::BF16),
        other => Err(format!("未知精度：{other}（可用：f32、f16、bf16）")),
    }
}
