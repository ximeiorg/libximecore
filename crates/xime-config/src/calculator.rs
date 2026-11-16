//! 计算器配置。
//!
//! 设置页「外观」里的「数学计算」开关写这一段。单独成段（而不是塞进
//! `StyleConfig`）是因为它跟外观无关：渲染层不读它，读它的是宿主服务进程。

use serde::{Deserialize, Serialize};

/// 计算器配置。
///
/// 顶层键会自动进 `XimeConfig::save_to` 的纳管列表，所以设置页保存时
/// 不需要额外改保存逻辑：与默认值不同的字段才会写进用户配置。
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CalculatorConfig {
    /// 数字开头自动进计算器影子缓冲（敲一串数字会被当作待上屏的数字）。
    ///
    /// 默认开启；只有显式写 `auto_detect: false` 才关闭。类型不对或没写
    /// 这一段都按开启处理——这条缺省语义在 `XimeYao` 侧也被依赖，
    /// 改名或改默认值要同步那边。
    #[serde(default = "default_auto_detect", deserialize_with = "deserialize_bool_or_default")]
    pub auto_detect: bool,
}

impl Default for CalculatorConfig {
    fn default() -> Self {
        Self {
            auto_detect: default_auto_detect(),
        }
    }
}

fn default_auto_detect() -> bool {
    true
}

/// 宽容解析布尔：显式 `false` 才关；写错类型（如 `auto_detect: nope`）回退默认
/// 开启，而不是让整份 `XimeConfig` 解析失败把用户配置一起带崩。与服务端
/// 读取端（XimeYao `auto_detect_from_yaml` 的 `as_bool().unwrap_or(true)`）同语义。
fn deserialize_bool_or_default<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_yaml::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(default_auto_detect()))
}

#[cfg(test)]
mod tests {
    use super::CalculatorConfig;

    /// 默认开启：没有这一段时行为跟历史上一致（数字开头就进影子缓冲）。
    #[test]
    fn defaults_to_enabled() {
        assert!(CalculatorConfig::default().auto_detect);
    }

    /// 显式 false 才关；写成别的类型（比如字符串）回退默认开启，
    /// 避免用户手改配置写错一个词就静默改变行为。
    #[test]
    fn parses_explicit_false_only() {
        let off: CalculatorConfig =
            serde_yaml::from_str("auto_detect: false").expect("应能解析");
        assert!(!off.auto_detect);

        let on: CalculatorConfig = serde_yaml::from_str("auto_detect: true").expect("应能解析");
        assert!(on.auto_detect);

        let empty: CalculatorConfig = serde_yaml::from_str("{}").expect("应能解析");
        assert!(empty.auto_detect);

        let wrong_type: CalculatorConfig =
            serde_yaml::from_str("auto_detect: nope").expect("类型不对应回退默认");
        assert!(wrong_type.auto_detect);
    }
}
