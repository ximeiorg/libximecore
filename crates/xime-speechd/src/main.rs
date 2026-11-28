//! 语音识别守护进程。
//!
//! 只做三件事：读参数 → 建服务 → 起 socket。平台托管（launchd / Windows 服务 /
//! systemd user unit）归宿主，不写在这里。
//!
//! ```text
//! xime-speechd --socket ~/Library/Application\ Support/XimeYi/speech.sock \
//!              --models ~/Library/Application\ Support/XimeYi/models/asr \
//!              [--device cpu|metal|cuda] [--dtype f32|f16] [--idle-secs 300] \
//!              [--speech-config ~/.config/xime/speech.toml]
//! ```
//!
//! **总开关**：`speech.toml` 的 `enabled = false` 是唯一的产品级关闭入口。
//! 关掉之后这个进程必须自己退出——模型常驻 1–2 GB，留在后台就是纯占用，
//! 而且拉起它的宿主可能已经退出（设置程序 / 输入法都可能不在跑），没人会来
//! 发 `Shutdown`。所以进程启动时先看一次开关，运行期间每 [`EXIT_POLL`] 再看
//! 一次，关了就卸引擎、删 socket、退出。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use xime_config::speech_config;
use xime_speech_service::{DType, DeviceKind, ServiceConfig, SpeechService};

/// 总开关轮询间隔。
///
/// 2 秒是「用户关掉开关后进程多久消失」与「重复读配置」之间的折中：
/// 每次只读一份几十字节的 TOML，代价可以忽略；进程退出不需要毫秒级。
const EXIT_POLL: Duration = Duration::from_secs(2);

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

    // 开关关着就别起服务：宿主可能没判（或用户直接改了文件），这里挡一道，
    // 省得白拉一个可能装着 1.9 GB 模型的进程。
    if switch_off(&args.speech_config) {
        eprintln!(
            "[xime-speechd] 语音输入已关闭（{}），不启动",
            args.speech_config.display()
        );
        return ExitCode::SUCCESS;
    }

    let provider = args.device.as_str().to_uppercase();
    let mut config = ServiceConfig::new(&args.models);
    config.device = args.device;
    config.dtype = args.dtype;
    config.idle_unload = Duration::from_secs(args.idle_secs);
    config.provider = provider;

    let service = Arc::new(SpeechService::new(config));
    eprintln!(
        "[xime-speechd] socket={} models={} speech.toml={}",
        args.socket,
        args.models.display(),
        args.speech_config.display()
    );
    spawn_exit_watcher(&args.socket, &args.speech_config, Arc::clone(&service));
    match xime_speech_service::transport::serve(&args.socket, service) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[xime-speechd] {error}");
            ExitCode::FAILURE
        }
    }
}

/// 后台线程：总开关一关就收摊。
///
/// 收尾与协议里的 `Request::Shutdown` 完全一致（卸引擎、删 socket、退出），
/// 只是触发源换成了配置文件——这样「谁拉起的进程」就不重要了。
fn spawn_exit_watcher(socket: &str, config: &Path, service: Arc<SpeechService>) {
    let socket = socket.to_string();
    let config = config.to_path_buf();
    let spawned = thread::Builder::new()
        .name("xime-speech-exit".to_string())
        .spawn(move || loop {
            thread::sleep(EXIT_POLL);
            if !switch_off(&config) {
                continue;
            }
            eprintln!(
                "[xime-speechd] 语音输入已关闭（{}），退出",
                config.display()
            );
            service.shutdown();
            #[cfg(unix)]
            {
                // 主动退出时删掉 socket：下次启动不用处理残留。
                let _ = std::fs::remove_file(&socket);
            }
            std::process::exit(0);
        });
    if let Err(error) = spawned {
        // 监视起不来不影响识别，只是关开关后要靠宿主发 Shutdown。
        eprintln!("[xime-speechd] 启动总开关监视失败：{error}");
    }
}

/// 总开关是否已关闭。
///
/// 文件不存在 / 解析失败一律按「开启」处理（[`speech_config::read_config_at`]
/// 的默认值）：配置坏掉就杀进程会让语音毫无征兆地消失，违背热路径的容错约定。
fn switch_off(config: &Path) -> bool {
    !speech_config::read_config_at(config).enabled
}

struct Args {
    /// Unix socket 路径；Windows 上是命名管道（可用 `xime-speech` 简写）。
    socket: String,
    models: PathBuf,
    device: DeviceKind,
    dtype: DType,
    idle_secs: u64,
    /// 总开关配置文件（`speech.toml`）。
    speech_config: PathBuf,
}

impl Args {
    const USAGE: &'static str = "\
用法: xime-speechd --socket <路径> --models <模型根目录> [选项]

选项:
  --device cpu|metal|cuda   推理后端（默认自动探测，GPU 不可用则 CPU）
  --dtype f32|f16|bf16      权重精度（默认 f32；Metal 建议 f16）
  --idle-secs <秒>           空闲多久卸载模型（默认 300）
  --speech-config <路径>     语音总开关配置（默认 <配置目录>/speech.toml）；
                             文件里 enabled=false 时进程自动退出
  --help                     显示这段说明

环境变量: XIME_SPEECH_SOCKET / XIME_SPEECH_MODELS 作为参数缺省值。";

    /// `Ok(None)` = 用户要 --help。
    fn parse(args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut socket = std::env::var("XIME_SPEECH_SOCKET").ok();
        let mut models = std::env::var_os("XIME_SPEECH_MODELS").map(PathBuf::from);
        // 不指定就探测：装了 cuda/metal 版本且显卡可用时走 GPU（流式窗口能开大），
        // 否则落回 CPU。显式 `--device` 仍然优先。
        let mut device: Option<String> = None;
        let mut dtype = "f32".to_string();
        let mut idle_secs = 300u64;
        let mut speech_config = speech_config::config_path();

        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            match flag.as_str() {
                "--help" | "-h" => return Ok(None),
                "--socket" => socket = Some(next_value(&mut args, &flag)?),
                "--models" => models = Some(next_value(&mut args, &flag)?.into()),
                "--device" => device = Some(next_value(&mut args, &flag)?),
                "--dtype" => dtype = next_value(&mut args, &flag)?,
                "--idle-secs" => {
                    idle_secs = next_value(&mut args, &flag)?
                        .parse()
                        .map_err(|_| "--idle-secs 需要整数".to_string())?;
                }
                "--speech-config" => speech_config = next_value(&mut args, &flag)?.into(),
                other => return Err(format!("未知参数：{other}")),
            }
        }

        Ok(Some(Self {
            socket: socket.ok_or("缺少 --socket（或 XIME_SPEECH_SOCKET）")?,
            models: models.ok_or("缺少 --models（或 XIME_SPEECH_MODELS）")?,
            device: match device.as_deref() {
                Some(value) => parse_device(value)?,
                None => DeviceKind::detect(),
            },
            dtype: parse_dtype(&dtype)?,
            idle_secs,
            speech_config,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 开关判定只看 `enabled`；文件缺失/损坏都按开启处理（不能误杀服务）。
    #[test]
    fn switch_off_only_when_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("speech.toml");

        assert!(!switch_off(&path), "文件不存在应按开启处理");

        let mut cfg = speech_config::SpeechConfig::default();
        speech_config::write_config_at(&path, &cfg);
        assert!(!switch_off(&path), "默认配置是开启");

        cfg.enabled = false;
        speech_config::write_config_at(&path, &cfg);
        assert!(switch_off(&path), "写 enabled=false 后应判定为关闭");

        cfg.enabled = true;
        speech_config::write_config_at(&path, &cfg);
        assert!(!switch_off(&path), "重新打开后应继续运行");

        std::fs::write(&path, "这不是 toml {{{").unwrap();
        assert!(!switch_off(&path), "损坏的配置按默认开启处理");
    }

    #[test]
    fn parses_speech_config_flag() {
        let args = Args::parse(
            [
                "--socket",
                "/tmp/speech.sock",
                "--models",
                "/tmp/models",
                "--speech-config",
                "/tmp/speech.toml",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap()
        .unwrap();
        assert_eq!(args.socket, "/tmp/speech.sock");
        assert_eq!(args.speech_config, PathBuf::from("/tmp/speech.toml"));
        assert_eq!(args.idle_secs, 300);
    }

    #[test]
    fn help_needs_no_arguments() {
        assert!(Args::parse(std::iter::once("--help".to_string()))
            .unwrap()
            .is_none());
    }
}
