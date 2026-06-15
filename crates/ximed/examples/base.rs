use ximed::serve;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let secret = parse_arg("--secret");
    serve(5023, secret).await
}

/// 从命令行参数中解析 `--key=value` 或 `--key value` 格式的值
fn parse_arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    // --key=value
    for arg in &args {
        if let Some(val) = arg.strip_prefix(&format!("{name}=")) {
            return Some(val.to_string());
        }
    }
    // --key value
    if let Some(pos) = args.iter().position(|a| a == name) {
        args.get(pos + 1).cloned()
    } else {
        None
    }
}
