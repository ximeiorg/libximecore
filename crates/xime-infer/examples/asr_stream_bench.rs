//! 流式重解码的成本曲线。
//!
//! 流式识别的做法是「逐秒编码 + 滚动窗口重解码」，解码成本随窗口长度线性
//! 增长（prefill 要重跑整段音频特征），所以窗口取多大、多久触发一次，必须
//! 用真实权重实测，不能拍脑袋。
//!
//! 用法：
//! ```text
//! cargo run --release -p xime-infer --example asr_stream_bench -- <模型目录> <wav>
//! ```
//!
//! 输出依次是：整段基线、逐秒编码单块成本、各窗口长度的重解码成本、带上文
//! context 后的额外开销。

use std::path::Path;
use std::time::Instant;

use candle_core::Tensor;
use xime_infer::asr::{AsrOptions, Qwen3Asr};

const RATE: usize = 16_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: asr_stream_bench <模型目录> <wav>");
        std::process::exit(2);
    }
    let dir = Path::new(&args[1]);
    let wav = Path::new(&args[2]);

    let t = Instant::now();
    let mut asr = Qwen3Asr::from_dir_with(dir, AsrOptions::default())?;
    println!("装载 {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    asr.warmup()?;
    println!("预热 {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    let pcm = xime_infer::asr::audio::load_wav_16k(wav, RATE as u32)?;
    let secs = pcm.len() / RATE;
    println!("音频 {:.2}s\n", pcm.len() as f64 / RATE as f64);

    // ---- 基线：整段一次 ----
    let out = asr.transcribe(&pcm)?;
    println!(
        "整段 {secs}s：编码 {:.0}ms + 解码 {:.0}ms = {:.0}ms，{} 文本 token",
        out.encode_ms,
        out.decode_ms,
        out.encode_ms + out.decode_ms,
        out.text_tokens
    );
    println!("  文本: {:?}\n", out.text);

    // ---- 逐秒编码（流式里每轮只付新增的那一块） ----
    let mut blocks: Vec<Tensor> = Vec::with_capacity(secs);
    let mut enc_total = 0.0;
    for i in 0..secs {
        let chunk = &pcm[i * RATE..(i + 1) * RATE];
        let t = Instant::now();
        blocks.push(asr.encode(chunk)?);
        enc_total += t.elapsed().as_secs_f64() * 1000.0;
    }
    println!(
        "逐秒编码：{secs} 块共 {enc_total:.0}ms，单块 {:.0}ms\n",
        enc_total / secs as f64
    );

    // ---- 窗口曲线：最近 N 秒重新解码 ----
    println!("窗口  音频token  解码ms  文本token  首字/秒");
    for win in [2usize, 4, 6, 8] {
        if win > secs {
            continue;
        }
        let feats = Tensor::cat(&blocks[secs - win..].iter().collect::<Vec<_>>(), 0)?;
        let (n_audio, _) = feats.dims2()?;
        let t = Instant::now();
        let out = asr.decode_embeds(&feats, None, win as f64 * 1000.0)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{win:>3}s  {n_audio:>8}  {ms:>6.0}  {:>9}  {:.1}",
            out.text_tokens,
            out.text_tokens as f64 / (ms / 1000.0)
        );
    }

    // ---- 带上文 context（system 段偏置）的额外开销 ----
    let win = secs.min(6);
    let feats = Tensor::cat(&blocks[secs - win..].iter().collect::<Vec<_>>(), 0)?;
    let context = "今天下午的会议改到三点半，地点还在原来的会议室，".repeat(2);
    let t = Instant::now();
    let out = asr.decode_embeds(&feats, Some(&context), win as f64 * 1000.0)?;
    println!(
        "\n带 context（{} 字）重解码：{:.0}ms，文本 {:?}",
        context.chars().count(),
        t.elapsed().as_secs_f64() * 1000.0,
        out.text
    );

    Ok(())
}
