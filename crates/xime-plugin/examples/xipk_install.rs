//! .xipk 插件包安装/卸载维护工具。
//!
//! 用法：
//! ```sh
//! cargo run -p xime-plugin --example xipk_install -- <plugins-root> <pkg.xipk> [pkg2.xipk ...]
//! cargo run -p xime-plugin --example xipk_install -- <plugins-root> --uninstall <id> [id2 ...]
//! cargo run -p xime-plugin --example xipk_install -- <plugins-root> --list
//! ```
//!
//! macOS 宿主数据目录：`~/Library/Application Support/XimeYi/plugins`

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("用法: xipk_install <plugins-root> <pkg.xipk... | --uninstall <id>... | --list>");
        return ExitCode::FAILURE;
    }
    let root = PathBuf::from(&args[0]);
    let manager = xime_plugin::PluginManager::new(&root);

    match args[1].as_str() {
        "--list" => {
            for record in manager.list() {
                println!(
                    "{}  {} v{}  type={}  enabled={}  state={:?}",
                    record.id,
                    record.name,
                    record.version,
                    record.plugin_type,
                    record.enabled,
                    record.state
                );
            }
        }
        "--uninstall" => {
            for id in &args[2..] {
                match manager.uninstall(id) {
                    Ok(()) => println!("已卸载 {id}"),
                    Err(e) => {
                        eprintln!("卸载 {id} 失败: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        _ => {
            for xipk in &args[1..] {
                let path = PathBuf::from(xipk);
                match manager.install_from_zip(&path, true) {
                    Ok(record) => println!(
                        "已安装 {} v{}（{}，enabled={}）",
                        record.id, record.version, record.plugin_type, record.enabled
                    ),
                    Err(e) => {
                        eprintln!("安装 {xipk} 失败: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
    }
    ExitCode::SUCCESS
}
