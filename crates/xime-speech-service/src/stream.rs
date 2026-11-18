//! 流式听写：分块编码 + 滚动窗口重解码 + 稳定前缀定稿。
//!
//! # 为什么不是「真流式」
//!
//! Qwen3-ASR 是编码器-解码器，不是流式模型：它没有 chunk 级的增量解码接口，
//! 一段音频进去、一整段文本出来。所以这里的「流式」是用**反复重解码**换来的：
//!
//! ```text
//! 麦克风 ──► 1 秒块 ──► 音频塔(增量, 每块只算一次) ──┐
//!                                                    ├─► 窗口特征 ──► 解码 ──► 文本
//!                          已定稿文本 ──► prompt system 段 ──┘
//! ```
//!
//! 每轮只重新解码、不重新编码，所以成本是「解码器的钱 × 触发频率」。实测
//! （RTX 4060 Ti）8 秒窗口一次 336 ms，1 秒新增足够付；纯 CPU 上 8 秒窗口要
//! 4 秒，追不上说话速度 —— 所以参数必须按后端能力选，见 [`StreamTuning`]。
//!
//! # 文本为什么不会来回跳
//!
//! 重解码的天然问题是每轮结果会微调（断句、标点、同音字），直接上屏会看到字
//! 在抖。这里用两点压制：
//!
//! 1. **稳定前缀**（LocalAgreement-N）：连续 N 轮结果一致的公共前缀才定稿，
//!    定稿的字立刻上屏、永不回改；
//! 2. **上文偏置**：已定稿文本塞进 Qwen3-ASR 的 system 段，让模型接着往下写，
//!    减少窗口开头把上半句重打一遍。
//!
//! 未定稿的部分作为 `partial` 给客户端画成 preedit（会整体替换，允许变）。

use std::time::{Duration, Instant};

use candle_core::Tensor;

use crate::engine::{AudioBlock, Recognizer};

/// 一个音频块的样本数：音频塔的 chunk 是 100 个 mel 帧 = 1 秒。
pub const CHUNK_SAMPLES: usize = 16_000;

/// 段间文本去重时最多回看的字符数。
const MAX_OVERLAP_CHARS: usize = 12;

/// 送进 prompt 的上文最多保留的字符数。
///
/// 已定稿文本是单调增长的，如果整段塞进 system 段，prompt 会随听写时长线性
/// 变长：几分钟后每轮解码从几百毫秒涨到几秒，最后甚至超出模型的上下文预算
/// 直接解码失败（表现为「越说越卡，然后一个字不出」）。模型只需要看到紧邻
/// 窗口的那一小段上文就能接着写，尾部 160 字足够。
const CONTEXT_CHARS: usize = 160;

/// 流式参数。按后端能力选 [`StreamTuning::gpu`] / [`StreamTuning::cpu`]。
#[derive(Debug, Clone)]
pub struct StreamTuning {
    /// 单段窗口上限（秒）。到顶就定稿整段、开新段。
    ///
    /// 上限来自模型本身：音频塔是 104 token 的滑动窗口注意力（13 秒），
    /// 超出这段的上下文本来就看不见；再加上重解码成本随窗口线性涨，所以
    /// 取 8 秒（GPU）/ 2 秒（CPU）。
    pub window_secs: f32,
    /// 两次重解码的最小间隔。要大于「一轮解码耗时」，否则会积压。
    pub min_interval: Duration,
    /// 连续多少轮结果一致才定稿（LocalAgreement-N）。
    pub agreement: u8,
    /// 静音多久算一句说完（提前定稿，不必等窗口到顶）。
    pub pause: Duration,
    /// 窗口到顶后额外再等多久，等一个句读再切段。
    ///
    /// 硬切会把词切断（实测出现过「打开轿车自动气」少了「阀」）；等到标点
    /// 再切能让段边界落在句子边界上。给 `window_secs + grace` 的硬上限兜底，
    /// 保证不会无限等下去（也保证窗口不超过模型 13 秒的注意力范围）。
    pub grace_secs: f32,
}

impl StreamTuning {
    /// GPU 后端：窗口长、触发密，接近逐字上屏。
    pub fn gpu() -> Self {
        Self {
            window_secs: 8.0,
            min_interval: Duration::from_millis(600),
            agreement: 2,
            pause: Duration::from_millis(400),
            grace_secs: 2.0,
        }
    }

    /// 纯 CPU：窗口短、触发疏，退化成「每两秒吐一小段」。
    ///
    /// 实测 CPU 上 2 秒窗口一轮 ≈ 1.2 秒（含编码），1.5 秒间隔刚好追上。
    pub fn cpu() -> Self {
        Self {
            window_secs: 2.0,
            min_interval: Duration::from_millis(1500),
            agreement: 2,
            pause: Duration::from_millis(400),
            // CPU 上每块都算钱，别为了等标点多跑几轮解码，到顶就切。
            grace_secs: 1.0,
        }
    }
}

/// 一轮处理的进展。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// 定稿文本变了（客户端应立刻上屏）。
    pub committed_changed: bool,
    /// 预览文本变了（客户端应重画 preedit）。
    pub partial_changed: bool,
}

impl Progress {
    pub fn any(self) -> bool {
        self.committed_changed || self.partial_changed
    }
}

/// 一次听写的流式会话。
pub struct StreamSession {
    tuning: StreamTuning,
    /// 当前段的音频特征，逐块追加。
    blocks: Vec<AudioBlock>,
    /// 不足 1 块的尾巴样本，等够 1 秒再编码。
    pending: Vec<f32>,
    /// 已定稿全文（跨段累积）。
    committed: String,
    /// 当前段最近一次解码结果。
    hyp: String,
    /// 上一轮的解码结果（LocalAgreement 用）。
    prev_hyp: String,
    /// 当前段里已经确认稳定的前缀。
    stable: String,
    /// `partial` 修订号，客户端据此判断要不要重画。
    partial_rev: u64,
    /// 上次解码的时刻（节流）。
    last_decode: Option<Instant>,
    /// 最近一次检测到有声音的时刻（停顿定稿用）。
    last_voice: Option<Instant>,
    /// 上一轮解码失败的原因（只报一次，避免刷屏）。
    error: Option<String>,
}

impl StreamSession {
    pub fn new(tuning: StreamTuning) -> Self {
        Self {
            tuning,
            blocks: Vec::new(),
            pending: Vec::new(),
            committed: String::new(),
            hyp: String::new(),
            prev_hyp: String::new(),
            stable: String::new(),
            partial_rev: 0,
            last_decode: None,
            last_voice: None,
            error: None,
        }
    }

    pub fn tuning(&self) -> &StreamTuning {
        &self.tuning
    }

    /// 已定稿全文：包含之前各段，以及当前段里已经确认稳定的前缀。
    pub fn committed(&self) -> &str {
        &self.committed
    }

    /// 上一轮解码的失败原因（成功后自动清空）。
    ///
    /// 由 worker 暴露给服务状态：解码一直失败时必须让用户看见「识别出错」，
    /// 否则表现就是「说半天不出字，也不知道是没录上还是没识别」。
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// 未定稿预览：当前段里还没稳定的尾巴，客户端画成 preedit。
    pub fn partial(&self) -> String {
        if self.hyp.starts_with(&self.stable) {
            self.hyp[self.stable.len()..].to_string()
        } else {
            self.hyp.clone()
        }
    }

    pub fn partial_rev(&self) -> u64 {
        self.partial_rev
    }

    /// 喂一段 16 kHz PCM 与当前输入电平，按需触发一轮重解码。
    ///
    /// `level` 用来判断停顿：连续静音超过 `pause` 就把当前段定稿。
    pub fn feed(
        &mut self,
        engine: &mut dyn Recognizer,
        pcm: &[f32],
        level: u8,
        now: Instant,
    ) -> Result<Progress, String> {
        let mut progress = Progress::default();
        if level > 0 {
            self.last_voice = Some(now);
        } else if self.last_voice.is_none() {
            // 还没说过话，别把开头的静音算成「停顿」。
            self.last_voice = Some(now);
        }

        self.pending.extend_from_slice(pcm);
        let mut grew = false;
        while self.pending.len() >= CHUNK_SAMPLES {
            let chunk: Vec<f32> = self.pending.drain(..CHUNK_SAMPLES).collect();
            let block = engine.encode_block(&chunk)?;
            self.blocks.push(block);
            grew = true;
        }
        if self.blocks.is_empty() {
            return Ok(progress);
        }

        let silence = self
            .last_voice
            .map(|at| now.saturating_duration_since(at))
            .unwrap_or_default();
        let paused = level == 0 && silence >= self.tuning.pause && !self.hyp.is_empty();

        // 没有新音频、也没到停顿：重解码同一个窗口纯属浪费（一轮要几百毫秒），
        // 直接返回让 worker 继续等下一块。
        if !grew && !paused {
            return Ok(progress);
        }

        // 节流：上一轮还没消化完就别再加一轮，否则延迟越滚越大。
        let due = match self.last_decode {
            None => true,
            Some(at) => now.saturating_duration_since(at) >= self.tuning.min_interval,
        };
        // 窗口到顶：优先等一个句读再切（硬切会切断词），最多多等 grace_secs。
        let blocks = self.blocks.len() as f32;
        let full = blocks >= self.tuning.window_secs;
        let hard = blocks >= self.tuning.window_secs + self.tuning.grace_secs;

        // 该收段了：无论节流与否都要跑最后一轮，然后定稿开新段。
        let closing = hard || (full && ends_sentence(&self.hyp)) || paused;
        if !due && !closing {
            return Ok(progress);
        }

        let window = self.window_tensor()?;
        let context = context_tail(&self.committed);
        let hyp = match engine.decode_window(&window, &context) {
            Ok(hyp) => {
                self.error = None;
                normalize(&hyp)
            }
            Err(error) => {
                self.error = Some(error);
                return Ok(progress);
            }
        };
        self.last_decode = Some(now);

        // 稳定前缀：连续 agreement 轮一致的部分才算数。确认即定稿、立刻上屏，
        // 这样客户端看到的 committed 单调变长，已落下的字不会被后续解码改掉。
        let advance = if self.tuning.agreement <= 1 {
            hyp.clone()
        } else {
            common_prefix(&hyp, &self.prev_hyp)
        };
        if advance.chars().count() > self.stable.chars().count()
            && advance.starts_with(&self.stable)
        {
            let fresh = advance[self.stable.len()..].to_string();
            if self.stable.is_empty() && !self.committed.is_empty() {
                // 新段的第一次推进：这里是两次独立解码的接缝，要按词边界补空格。
                // 段内的后续推进来自同一轮解码结果，直接拼即可。
                if needs_space(self.committed.chars().last(), fresh.chars().next()) {
                    self.committed.push(' ');
                }
            }
            self.committed.push_str(&fresh);
            self.stable = advance;
            progress.committed_changed = true;
        }

        if hyp != self.hyp {
            self.hyp = hyp;
            self.partial_rev += 1;
            progress.partial_changed = true;
        }
        self.prev_hyp = self.hyp.clone();

        if closing {
            let tail = self.take_segment();
            if !tail.is_empty() {
                self.append_committed(&tail);
                progress.committed_changed = true;
            }
            self.partial_rev += 1;
            progress.partial_changed = true;
        }

        Ok(progress)
    }

    /// 松手：把尾巴补零成整块、跑最后一轮，返回定稿全文。
    pub fn finish(&mut self, engine: &mut dyn Recognizer) -> Result<String, String> {
        if !self.pending.is_empty() {
            // 尾部不足 1 秒也要过一遍音频塔（mel 内部会补到整秒）。
            let mut tail = std::mem::take(&mut self.pending);
            tail.resize(CHUNK_SAMPLES, 0.0);
            let block = engine.encode_block(&tail)?;
            self.blocks.push(block);
        }
        if !self.blocks.is_empty() {
            let window = self.window_tensor()?;
            let context = context_tail(&self.committed);
            if let Ok(hyp) = engine.decode_window(&window, &context) {
                let hyp = normalize(&hyp);
                if hyp.chars().count() >= self.hyp.chars().count() {
                    self.hyp = hyp;
                }
            }
            let tail = self.take_segment();
            if !tail.is_empty() {
                self.append_committed(&tail);
            }
        }
        let _ = std::mem::take(&mut self.stable);
        self.partial_rev += 1;
        Ok(self.committed.clone())
    }

    /// 取消：丢掉一切，返回已定稿的部分（是否采用由调用方决定）。
    pub fn cancel(&mut self) {
        self.blocks.clear();
        self.pending.clear();
        self.hyp.clear();
        self.prev_hyp.clear();
        self.stable.clear();
        self.partial_rev += 1;
    }

    fn window_tensor(&self) -> Result<Tensor, String> {
        let refs: Vec<&Tensor> = self.blocks.iter().collect();
        Tensor::cat(&refs, 0).map_err(|e| format!("拼接音频特征失败：{e}"))
    }

    /// 结束当前段：返回该段还有多少文本没上屏，并清空段状态。
    fn take_segment(&mut self) -> String {
        let hyp = std::mem::take(&mut self.hyp);
        let stable = std::mem::take(&mut self.stable);
        self.blocks.clear();
        self.prev_hyp.clear();
        self.last_decode = None;
        if hyp.starts_with(&stable) {
            // 稳定前缀在确认时就已经进了 committed，这里只补尾巴。
            hyp[stable.len()..].to_string()
        } else {
            // 模型把已定稿的部分又改写了：只能靠文本重叠兜底去重。
            strip_overlap(&self.committed, &hyp)
        }
    }

    fn append_committed(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // 段与段之间是两次独立解码的接缝：英文/中英混排要在词边界补空格，
        // 否则会出现 `front.Center` 这种粘连。中文两侧都是非 ASCII，不补。
        if needs_space(self.committed.chars().last(), text.chars().next()) {
            self.committed.push(' ');
        }
        self.committed.push_str(text);
    }
}

/// 取上文末尾 `CONTEXT_CHARS` 个字符作为 prompt 上下文。
fn context_tail(committed: &str) -> String {
    let count = committed.chars().count();
    if count <= CONTEXT_CHARS {
        return committed.to_string();
    }
    committed.chars().skip(count - CONTEXT_CHARS).collect()
}

/// 拼接两段文本时是否需要在中间补空格。
///
/// 只在两边都是「ASCII 词素」时补：字母数字，或句读标点后接字母数字
/// （英文习惯 `end. Next`）。中文标点、汉字一律不触发。
fn needs_space(prev: Option<char>, next: Option<char>) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric();
    let break_after = |c: char| matches!(c, '.' | ',' | '!' | '?' | ';' | ':' | ')' | '"' | '\'');
    let break_before = |c: char| matches!(c, '(' | '"' | '\'');
    match (prev, next) {
        (Some(a), Some(b)) => (word(a) || break_after(a)) && (word(b) || break_before(b)),
        _ => false,
    }
}

/// 规范化模型输出：去首尾空白、把连续空白压成一个空格。
///
/// Qwen3-ASR 偶尔会在中文之间吐空格，直接上屏看起来像被拆过。
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim_end().to_string()
}

/// 文本是否停在句子边界上（切段点）。
fn ends_sentence(text: &str) -> bool {
    matches!(
        text.chars().last(),
        Some('。' | '！' | '？' | '；' | '，' | '.' | '!' | '?' | ';' | ',')
    )
}

/// 两个字符串的公共前缀（按字符）。
fn common_prefix(a: &str, b: &str) -> String {
    let mut out = String::new();
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            break;
        }
        out.push(x);
    }
    out
}

/// 去掉 `hyp` 开头与 `prev` 结尾重复的部分。
///
/// 段与段之间没有音频重叠，但模型带着上文 context 解码时，常把上一段的尾巴
/// 再吐一遍。只在匹配长度 ≥2 时裁剪，避免把正常的「的」「了」误删。
fn strip_overlap(prev: &str, hyp: &str) -> String {
    let p: Vec<char> = prev.chars().collect();
    let h: Vec<char> = hyp.chars().collect();
    let max = p.len().min(h.len()).min(MAX_OVERLAP_CHARS);
    for k in (2..=max).rev() {
        if p[p.len() - k..] == h[..k] {
            return h[k..].iter().collect();
        }
    }
    hyp.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假引擎：按预设序列吐文本，每块编码成一个占位张量。
    struct FakeEngine {
        replies: Vec<String>,
        calls: usize,
        fail: bool,
    }

    impl FakeEngine {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: replies.iter().map(|s| s.to_string()).collect(),
                calls: 0,
                fail: false,
            }
        }
    }

    impl Recognizer for FakeEngine {
        fn transcribe(&mut self, _pcm: &[f32]) -> Result<String, String> {
            Ok(self.replies.first().cloned().unwrap_or_default())
        }

        fn supports_streaming(&self) -> bool {
            true
        }

        fn encode_block(&self, _pcm: &[f32]) -> Result<AudioBlock, String> {
            if self.fail {
                return Err("编码失败".to_string());
            }
            Ok(Tensor::zeros((13, 8), candle_core::DType::F32, &candle_core::Device::Cpu).unwrap())
        }

        fn decode_window(
            &mut self,
            _window: &AudioBlock,
            _context: &str,
        ) -> Result<String, String> {
            let reply = self
                .replies
                .get(self.calls)
                .cloned()
                .unwrap_or_else(|| self.replies.last().cloned().unwrap_or_default());
            self.calls += 1;
            Ok(reply)
        }
    }

    fn tuning() -> StreamTuning {
        StreamTuning {
            window_secs: 2.0,
            min_interval: Duration::from_millis(0),
            agreement: 2,
            pause: Duration::from_millis(300),
            grace_secs: 1.0,
        }
    }

    fn speech(n: usize) -> Vec<f32> {
        vec![0.1; n]
    }

    #[test]
    fn agreement_defers_commit_until_two_rounds_match() {
        let mut engine = FakeEngine::new(&["今天下午开会", "今天下午开会"]);
        let mut session = StreamSession::new(tuning());
        let now = Instant::now();

        // 第一轮：只有一次结果，公共前缀为空 → 什么都不能定稿。
        let p = session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, now)
            .unwrap();
        assert!(!p.committed_changed, "单轮结果不足以定稿");
        assert_eq!(session.partial(), "今天下午开会");

        // 第二轮结果一致 → 公共前缀全部稳定。
        let p = session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                now + Duration::from_secs(1),
            )
            .unwrap();
        assert!(p.committed_changed);
        assert_eq!(session.committed(), "今天下午开会");
    }

    #[test]
    fn committed_prefix_is_removed_from_partial() {
        let mut engine = FakeEngine::new(&["今天下午开会", "今天下午开会"]);
        let mut session = StreamSession::new(tuning());
        let t0 = Instant::now();
        session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, t0)
            .unwrap();
        session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                t0 + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(session.committed(), "今天下午开会");
        assert_eq!(session.partial(), "", "已定稿部分不该再出现在预览里");
    }

    #[test]
    fn silence_closes_segment_and_keeps_text() {
        let mut engine = FakeEngine::new(&["好的", "好的"]);
        // 窗口放大，确保段结束只可能由停顿触发（否则 2 块就到顶了）。
        let mut session = StreamSession::new(StreamTuning {
            window_secs: 10.0,
            ..tuning()
        });
        let t0 = Instant::now();
        session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, t0)
            .unwrap();
        session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                t0 + Duration::from_secs(1),
            )
            .unwrap();
        // 静音超过 pause → 段结束；文本早已定稿，不会再重复追加。
        let p = session
            .feed(&mut engine, &[], 0, t0 + Duration::from_secs(2))
            .unwrap();
        assert!(p.partial_changed, "段结束要把预览清干净");
        assert_eq!(session.committed(), "好的");
        assert_eq!(session.partial(), "");
        assert!(session.blocks.is_empty(), "段结束后窗口应清空");
    }

    #[test]
    fn window_limit_waits_for_sentence_then_hard_cuts() {
        let mut engine = FakeEngine::new(&["一二三四五"]);
        let mut session = StreamSession::new(tuning());
        let t0 = Instant::now();
        session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, t0)
            .unwrap();
        // 到顶但没标点：先不收段，等一句说完再切（硬切会切断词）。
        session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                t0 + Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(session.committed(), "一二三四五");
        assert_eq!(session.blocks.len(), 2, "到顶无标点时应继续攒窗口");
        // 超过 grace 仍没标点：硬切，不能无限等。
        session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                t0 + Duration::from_secs(2),
            )
            .unwrap();
        assert!(session.blocks.is_empty(), "超过 grace 必须收段");
    }

    #[test]
    fn window_limit_cuts_early_on_sentence_end() {
        let mut engine = FakeEngine::new(&["一二三四五。", "一二三四五。"]);
        let mut session = StreamSession::new(tuning());
        let t0 = Instant::now();
        session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, t0)
            .unwrap();
        // 第二轮 hyp 以句号结尾 → 到顶即切，不必等满 grace。
        session
            .feed(
                &mut engine,
                &speech(CHUNK_SAMPLES),
                50,
                t0 + Duration::from_secs(1),
            )
            .unwrap();
        assert!(session.blocks.is_empty(), "句末到顶应立刻收段");
        assert_eq!(session.committed(), "一二三四五。");
    }

    #[test]
    fn strip_overlap_removes_repeated_tail() {
        assert_eq!(strip_overlap("今天下午开会", "开会改到三点"), "改到三点");
        assert_eq!(strip_overlap("今天下午开会", "明天见"), "明天见");
        // 只重复一个字不裁剪，避免误删。
        assert_eq!(strip_overlap("好的", "的确如此"), "的确如此");
    }

    #[test]
    fn normalize_collapses_spaces() {
        assert_eq!(normalize(" 你 好  世界 "), "你 好 世界");
    }

    #[test]
    fn segment_join_inserts_space_only_between_ascii_words() {
        assert!(needs_space(Some('t'), Some('C')), "英文词之间要补空格");
        assert!(needs_space(Some('.'), Some('C')), "句号后接英文要补");
        assert!(!needs_space(Some('会'), Some('改')), "中文之间不补");
        assert!(
            !needs_space(Some('会'), Some('C')),
            "中文接英文不补，避免中文里出现孤立空格"
        );
        assert!(!needs_space(None, Some('C')), "开头不补");
        assert!(!needs_space(Some('t'), None), "结尾不补");
    }

    #[test]
    fn encode_error_surfaces_once() {
        let mut engine = FakeEngine::new(&["x"]);
        engine.fail = true;
        let mut session = StreamSession::new(tuning());
        let err = session
            .feed(&mut engine, &speech(CHUNK_SAMPLES), 50, Instant::now())
            .unwrap_err();
        assert!(err.contains("编码失败"), "{err}");
    }
}
