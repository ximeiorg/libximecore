//! 本地 ASR 模型注册表（对齐 Android xime speech 的 `AsrModelRegistry`）。
//!
//! 两端共用同一批 ModelScope 模型包（都是 sherpa-onnx 导出格式）：
//! 注册表是宿主侧的**权威文件角色映射**——扩展商店条目只提供下载与展示
//! 信息，按 id 在这里查到 profile 才能装载；索引未收录的内置 profile 仍
//! 可被选中使用。未知 id（索引新增、本端未随版更新）回退默认文件布局，
//! 保证按通用命名发布的 zipformer2 模型仍可加载（目录名保留原 id）。

/// 一个本地流式模型的角色清单：模型目录内四个文件的相对名 + 下载信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AsrModelProfile {
    /// 模型 id（= 模型目录名，与扩展商店/宿主持久化的选中值一致）。
    pub id: String,
    /// 展示名（设置页）。
    pub name: String,
    /// 一句话描述（设置页）。
    pub description: String,
    /// 语言标签："zh" / "zh-en"。
    pub language: String,
    /// 下载包大小（如实标注，仅展示）。
    pub size: String,
    /// tar.bz2 包下载地址（ModelScope）。
    pub download_url: String,
    /// 编码器 onnx 文件名。
    pub encoder_file: String,
    /// 解码器 onnx 文件名。
    pub decoder_file: String,
    /// 联接器 onnx 文件名。
    pub joiner_file: String,
    /// 词表文件名（X-ASR 为 5000 BPE；zipformer-zh 为字级）。
    pub tokens_file: String,
}

/// 内置模型注册表（新模型在此追加；默认模型见 [`AsrModelRegistry::default_profile`]）。
pub struct AsrModelRegistry;

impl AsrModelRegistry {
    /// X-ASR-zh-en 流式模型（480ms chunk，int8，带标点变体）。
    ///
    /// 基于 icefall/Zipformer transducer，约 100 万小时中英文数据训练，
    /// 5000 BPE 词表（▁ 剥离与 `<0xNN>` 字节回退由 sherpa-onnx 解码器处理）。
    /// 词表含标点 token：输出带中英文标点，英文大小写混合——听写文本直接可用，
    /// 因此是本端默认模型（Android 默认是 zipformer-zh-int8，两端注册表同表、
    /// 各自默认可不同）。
    ///
    /// `name` 是**给用户看的**（设置页下拉/状态行），别放模型 id 那种工程名；
    /// 技术出处写进 `description` 或注释。
    fn x_asr_480ms_zh_en_punct_int8() -> AsrModelProfile {
        AsrModelProfile {
            id: "x-asr-480ms-zh-en-punct-int8".into(),
            name: "中英混输（自动标点）".into(),
            description: "说中文、说英文都行，边说边出字，结果自动加标点；日常输入推荐这个".into(),
            language: "zh-en".into(),
            size: "133.90MB".into(),
            download_url: "https://www.modelscope.cn/models/adaada88/sherpa-onnx-x-asr-480ms-streaming-zipformer-transducer-zh-en-punct-int8/resolve/master/sherpa-onnx-x-asr-480ms-streaming-zipformer-transducer-zh-en-punct-int8-2026-06-05.tar.bz2".into(),
            encoder_file: "encoder.int8.onnx".into(),
            decoder_file: "decoder.onnx".into(),
            joiner_file: "joiner.int8.onnx".into(),
            tokens_file: "tokens.txt".into(),
        }
    }

    /// sherpa-onnx 官方中文流式 zipformer（int8，字级词表，无标点输出）。
    fn zipformer_zh_int8() -> AsrModelProfile {
        AsrModelProfile {
            id: "zipformer-zh-int8".into(),
            name: "纯中文（不带标点）".into(),
            description: "只识别中文，输出不带标点；说的全是中文时可选它".into(),
            language: "zh".into(),
            size: "132.63MB".into(),
            download_url: "https://www.modelscope.cn/models/bikeand/asr/resolve/master/sherpa-onnx-streaming-zipformer-zh-int8-2025-06-30.tar.bz2".into(),
            encoder_file: "encoder.int8.onnx".into(),
            decoder_file: "decoder.onnx".into(),
            joiner_file: "joiner.int8.onnx".into(),
            tokens_file: "tokens.txt".into(),
        }
    }

    /// 推荐模型 id（= 首个内置模型）：设置页给这一个挂「推荐」标记。
    pub fn recommended_id() -> String {
        Self::profiles()
            .first()
            .map(|profile| profile.id.clone())
            .unwrap_or_default()
    }

    /// 全部内置模型适配（对齐 Android `AsrModelRegistry.profiles`，首项即默认）。
    pub fn profiles() -> Vec<AsrModelProfile> {
        vec![Self::x_asr_480ms_zh_en_punct_int8(), Self::zipformer_zh_int8()]
    }

    /// 默认模型：X-ASR 带标点（输入法听写要的是「直接可用的文本」）。
    pub fn default_profile() -> AsrModelProfile {
        Self::x_asr_480ms_zh_en_punct_int8()
    }

    /// 按 id 查找内置适配。
    pub fn find_by_id(id: &str) -> Option<AsrModelProfile> {
        Self::profiles().into_iter().find(|p| p.id == id)
    }

    /// 按 id 取适配；未知 id（索引新增、本端未随之更新）回退默认文件布局
    /// 但保留原 id 作目录名——按通用命名发布的 zipformer2 模型仍可加载。
    pub fn profile_or_default(id: &str) -> AsrModelProfile {
        Self::find_by_id(id).unwrap_or_else(|| {
            let mut profile = Self::default_profile();
            profile.id = id.to_string();
            profile
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_both_android_profiles() {
        let ids: Vec<String> = AsrModelRegistry::profiles().iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, vec!["x-asr-480ms-zh-en-punct-int8", "zipformer-zh-int8"]);
    }

    #[test]
    fn default_is_x_asr_punct() {
        // 与 Android 默认（zipformer-zh-int8）刻意不同：听写文本要带标点可直接上屏
        assert_eq!(AsrModelRegistry::default_profile().id, "x-asr-480ms-zh-en-punct-int8");
        assert_eq!(AsrModelRegistry::profiles()[0].id, AsrModelRegistry::default_profile().id);
    }

    #[test]
    fn find_by_id_hits_builtin() {
        let p = AsrModelRegistry::find_by_id("zipformer-zh-int8");
        assert!(p.is_some());
        assert_eq!(p.unwrap().language, "zh");
        assert!(AsrModelRegistry::find_by_id("no-such-model").is_none());
    }

    #[test]
    fn unknown_id_falls_back_to_default_layout_with_original_id() {
        let p = AsrModelRegistry::profile_or_default("future-model-id");
        assert_eq!(p.id, "future-model-id");
        // 文件布局回退默认（X-ASR 命名），目录名用原 id
        assert_eq!(p.encoder_file, AsrModelRegistry::default_profile().encoder_file);
        // 已知 id 原样返回
        let known = AsrModelRegistry::profile_or_default("zipformer-zh-int8");
        assert_eq!(known.language, "zh");
    }

    #[test]
    fn profiles_carry_download_and_file_roles() {
        for p in AsrModelRegistry::profiles() {
            assert!(p.download_url.starts_with("https://www.modelscope.cn/"), "{} url", p.id);
            assert!(p.download_url.ends_with(".tar.bz2"), "{} archive", p.id);
            assert!(!p.encoder_file.is_empty());
            assert!(!p.decoder_file.is_empty());
            assert!(!p.joiner_file.is_empty());
            assert_eq!(p.tokens_file, "tokens.txt");
        }
    }
}
