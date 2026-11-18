//! 本地传输层：Unix socket / Windows 命名管道上的 JSON-line 服务端与客户端。
//!
//! 三平台用同一套实现（`interprocess` 把 Unix domain socket 与命名管道抹平）：
//!
//! | 平台 | endpoint 形态 | 权限 |
//! | --- | --- | --- |
//! | macOS / Linux | 文件路径，例如 `…/speech.sock` | 建好后 chmod 0600 |
//! | Windows | 管道名，例如 `\\.\pipe\xime-speech` | 默认 DACL（创建者 + SYSTEM） |
//!
//! 不监听 TCP：这个进程能开麦克风、能读模型，暴露到网络等于给同网段一个录音器。

use std::io::{BufRead, BufReader, Write};

use interprocess::local_socket::{prelude::*, Name, Stream};

#[cfg(feature = "server")]
use std::path::Path;
#[cfg(feature = "server")]
use std::sync::Arc;
#[cfg(feature = "server")]
use std::thread;
#[cfg(feature = "server")]
use std::time::Duration;
#[cfg(feature = "server")]
use interprocess::local_socket::ListenerOptions;

use crate::protocol::{decode_line, encode_line, Request, Response};
#[cfg(feature = "server")]
use crate::service::SpeechService;

/// 处理一条请求，返回 `(响应, 是否要求退出)`。
///
/// 抽成纯函数：不碰 socket，协议行为可以直接单测。
#[cfg(feature = "server")]
pub fn dispatch(service: &Arc<SpeechService>, request: &Request) -> (Response, bool) {
    match request {
        Request::Status => (Response::with_status(service.status()), false),
        Request::Models => {
            let models = service.models();
            (Response::with_models(models, service.status()), false)
        }
        Request::Select { model_id } => match service.select(model_id) {
            Ok(status) => (Response::with_status(status), false),
            Err(error) => (Response::error(error), false),
        },
        Request::Import { path } => match service.import(path) {
            Ok((id, status)) => {
                let mut response = Response::with_status(status);
                response.message = Some(format!("已导入「{id}」"));
                (response, false)
            }
            Err(error) => (Response::error(error), false),
        },
        Request::Remove { model_id } => match service.remove(model_id) {
            Ok(status) => {
                let mut response = Response::with_status(status);
                response.message = Some(format!("已删除「{model_id}」"));
                (response, false)
            }
            Err(error) => (Response::error(error), false),
        },
        Request::ListenStart => match service.listen_start() {
            Ok(status) => (Response::with_status(status), false),
            Err(error) => (Response::error(error), false),
        },
        Request::ListenStop => match service.listen_stop() {
            Ok(status) => {
                let mut response = Response::with_status(status);
                response.accepted = Some(true);
                (response, false)
            }
            Err(error) => (Response::error(error), false),
        },
        Request::Shutdown => (Response::with_status(service.status()), true),
    }
}

/// endpoint 字符串 → 平台本地 socket 名。
fn endpoint_name(endpoint: &str) -> Result<Name<'static>, String> {
    #[cfg(unix)]
    {
        endpoint
            .to_fs_name::<interprocess::local_socket::GenericFilePath>()
            .map(|name| name.into_owned())
            .map_err(|e| format!("非法的 socket 路径（{endpoint}）：{e}"))
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::NamedPipe;
        // 允许宿主只传名字（`xime-speech`），自动补成 `\\.\pipe\xime-speech`。
        let full = if endpoint.starts_with(r"\\.\pipe\") {
            endpoint.to_string()
        } else {
            format!(r"\\.\pipe\{}", endpoint.trim_start_matches(r"\\?\pipe\"))
        };
        full.to_fs_name::<NamedPipe>()
            .map(|name| name.into_owned())
            .map_err(|e| format!("非法的管道名（{endpoint}）：{e}"))
    }
}

/// 启动服务端并阻塞（直到进程退出）。
#[cfg(feature = "server")]
pub fn serve(endpoint: &str, service: Arc<SpeechService>) -> Result<(), String> {
    #[cfg(unix)]
    {
        // 残留 socket：能连上说明已有实例在跑；连不上就是上次崩溃留下的，删掉重建。
        if Path::new(endpoint).exists() {
            let name = endpoint_name(endpoint)?;
            if Stream::connect(name).is_ok() {
                return Err(format!("已有语音服务在运行：{endpoint}"));
            }
            let _ = std::fs::remove_file(endpoint);
        }
        if let Some(parent) = Path::new(endpoint).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建 socket 目录失败：{e}"))?;
        }
    }

    let listener = ListenerOptions::new()
        .name(endpoint_name(endpoint)?)
        .create_sync()
        .map_err(|e| format!("监听失败（{endpoint}）：{e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600));
    }

    // 空闲卸载：每 5 秒检查一次，超时就把引擎内存还回去。
    let maintainer = Arc::clone(&service);
    thread::Builder::new()
        .name("xime-speech-maintain".to_string())
        .spawn(move || loop {
            thread::sleep(Duration::from_secs(5));
            maintainer.maintain();
        })
        .map_err(|e| format!("启动维护线程失败：{e}"))?;

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let service = Arc::clone(&service);
                let endpoint = endpoint.to_string();
                thread::Builder::new()
                    .name("xime-speech-conn".to_string())
                    .spawn(move || handle_connection(stream, service, endpoint))
                    .ok();
            }
            Err(error) => eprintln!("[xime-speechd] accept 失败：{error}"),
        }
    }
    Ok(())
}

#[cfg(feature = "server")]
fn handle_connection(stream: Stream, service: Arc<SpeechService>, endpoint: String) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("[xime-speechd] 读取失败：{error}");
                break;
            }
        }
        if line.trim().is_empty() {
            continue;
        }
        let request = match decode_line::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                if write_line(&mut reader, &Response::error(error)).is_err() {
                    break;
                }
                continue;
            }
        };
        let (response, shutdown) = dispatch(&service, &request);
        if write_line(&mut reader, &response).is_err() {
            break;
        }
        if shutdown {
            service.shutdown();
            #[cfg(unix)]
            {
                // 主动退出时把 socket 文件删掉：下次启动不用处理残留。
                let _ = std::fs::remove_file(&endpoint);
            }
            std::process::exit(0);
        }
    }
}

#[cfg(feature = "server")]
fn write_line<R: Write>(writer: &mut BufReader<R>, response: &Response) -> std::io::Result<()> {
    let line = encode_line(response).map_err(std::io::Error::other)?;
    writer.get_mut().write_all(line.as_bytes())?;
    writer.get_mut().flush()
}

/// 客户端：一条连接、一问一答。
pub struct Client {
    reader: BufReader<Stream>,
}

impl Client {
    pub fn connect(endpoint: &str) -> Result<Self, String> {
        let stream = Stream::connect(endpoint_name(endpoint)?)
            .map_err(|e| format!("连接语音服务失败（{endpoint}）：{e}"))?;
        Ok(Self {
            reader: BufReader::new(stream),
        })
    }

    pub fn call(&mut self, request: &Request) -> Result<Response, String> {
        let line = encode_line(request)?;
        self.reader
            .get_mut()
            .write_all(line.as_bytes())
            .map_err(|e| format!("发送请求失败：{e}"))?;
        self.reader.get_mut().flush().ok();
        let mut response = String::new();
        self.reader
            .read_line(&mut response)
            .map_err(|e| format!("读取响应失败：{e}"))?;
        if response.is_empty() {
            return Err("语音服务断开了连接".to_string());
        }
        decode_line(&response)
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::engine::Recognizer;
    use crate::service::ServiceConfig;

    struct Fake;

    impl Recognizer for Fake {
        fn transcribe(&mut self, _pcm: &[f32]) -> Result<String, String> {
            Ok("测试文本".to_string())
        }
    }

    fn setup() -> (tempfile::TempDir, Arc<SpeechService>) {
        let tmp = tempfile::tempdir().unwrap();
        // 模型根目录单独一层：临时目录里还会放导入源，不能被当成模型扫进来。
        let root = tmp.path().join("models");
        let dir = root.join("qwen3-asr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        std::fs::write(dir.join("tokenizer.json"), "{}").unwrap();
        std::fs::write(dir.join("model.safetensors"), vec![0u8; 8]).unwrap();
        let service = Arc::new(SpeechService::with_recognizer(
            ServiceConfig::new(&root),
            Box::new(Fake),
        ));
        (tmp, service)
    }

    #[test]
    fn status_returns_models_and_shutdown_flag() {
        let (_tmp, service) = setup();
        let (response, shutdown) = dispatch(&service, &Request::Status);
        assert!(response.ok);
        assert!(!shutdown);
        let status = response.status.unwrap();
        assert_eq!(status.models.len(), 1);
        assert_eq!(status.model_id, "qwen3-asr");

        let (_response, shutdown) = dispatch(&service, &Request::Shutdown);
        assert!(shutdown);
    }

    #[test]
    fn select_unknown_model_reports_error_not_panic() {
        let (_tmp, service) = setup();
        let (response, shutdown) = dispatch(
            &service,
            &Request::Select {
                model_id: "nope".into(),
            },
        );
        assert!(!response.ok);
        assert!(!shutdown);
        assert!(response.error.unwrap().contains("不存在"));
    }

    #[test]
    fn import_then_remove_roundtrip() {
        let (tmp, service) = setup();
        let src = tmp.path().join("incoming").join("second-model");
        std::fs::create_dir_all(&src).unwrap();
        for file in ["config.json", "tokenizer.json"] {
            std::fs::write(src.join(file), "{}").unwrap();
        }
        std::fs::write(src.join("model.safetensors"), vec![0u8; 8]).unwrap();

        let (response, _) = dispatch(
            &service,
            &Request::Import {
                path: src.display().to_string(),
            },
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.status.unwrap().models.len(), 2);

        let (response, _) = dispatch(
            &service,
            &Request::Remove {
                model_id: "second-model".into(),
            },
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.status.unwrap().models.len(), 1);
    }

    /// 真起一个 socket，客户端走完整 JSON-line 往返（三平台同一条路径）。
    #[cfg(unix)]
    #[test]
    fn socket_roundtrip_and_shutdown_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let endpoint = tmp.path().join("speech.sock");
        let endpoint = endpoint.display().to_string();

        let (_models, service) = setup();
        let server_service = Arc::clone(&service);
        let server_endpoint = endpoint.clone();
        let handle = std::thread::spawn(move || {
            let _ = serve(&server_endpoint, server_service);
        });

        // 等 socket 建好（serve 是阻塞的，所以轮询文件存在性）。
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !Path::new(&endpoint).exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(Path::new(&endpoint).exists(), "socket 没建起来");

        let mut client = Client::connect(&endpoint).unwrap();
        let response = client.call(&Request::Status).unwrap();
        assert!(response.ok);
        assert_eq!(response.status.unwrap().models.len(), 1);

        let response = client.call(&Request::Models).unwrap();
        assert_eq!(response.models.unwrap().len(), 1);
        drop(client);
        drop(handle);
        // 注意：不在这里发 Shutdown——它会 exit(0) 整个测试进程。
        // 清理逻辑由守护进程冒烟脚本验证（见 README）。
    }
}
