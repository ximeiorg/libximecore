"""用 transformers 的 Whisper 参考实现给 Rust 侧做数值对拍。

三段分别比，错在哪一层能直接看出来：

1. 前处理 —— 与 `WhisperFeatureExtractor` 输出的 `input_features` 逐点比；
2. 编码器 —— 与 `model.model.encoder(input_features).last_hidden_state` 逐点比；
3. 解码 prompt + 贪心文本 —— 与 `model.generate(...)` 比。

用法：

    # 先在 Rust 侧导出中间量（语种要与下面 --language 一致）
    XIME_ASR_DUMP=/tmp/whisper_dump XIME_ASR_DUMP_IDS=/tmp/whisper_dump/ids.txt \\
      cargo run --release -p xime-infer --example whisper_transcribe \\
      -- <模型目录> <wav> en

    python tools/whisper_reference.py <模型目录> <wav> --dump /tmp/whisper_dump --language en

依赖：`pip install torch transformers`（CPU 版即可）。
"""

from __future__ import annotations

import argparse
import os

import numpy as np
import torch
from transformers import WhisperForConditionalGeneration, WhisperProcessor


def read_bin(path: str) -> np.ndarray:
    shape = tuple(int(x) for x in open(path + "_shape.txt").read().split())
    data = np.fromfile(path + "_f32.bin", dtype=np.float32)
    return data.reshape(shape)


def stats(name: str, ref: np.ndarray, got: np.ndarray) -> bool:
    """按相对 RMS 和余弦相似度判定（与 fun_asr_reference.py 同一套口径）。"""
    ref = ref.astype(np.float64).ravel()
    got = got.astype(np.float64).ravel()
    if ref.shape != got.shape:
        print(f"[FAIL] {name}: 形状 {got.shape} != 参考 {ref.shape}")
        return False
    diff = got - ref
    rel = float(np.sqrt((diff**2).mean()) / max(np.sqrt((ref**2).mean()), 1e-12))
    denom = float(np.linalg.norm(ref) * np.linalg.norm(got)) or 1.0
    cos = float(np.dot(ref, got) / denom)
    worst = float(np.abs(diff).max())
    ok = rel < 2e-3 and cos > 0.9995
    print(
        f"[{'ok' if ok else 'FAIL'}] {name}: 相对 RMS {rel:.2e}，余弦 {cos:.6f}，"
        f"最大绝对差 {worst:.2e}"
    )
    return ok


def read_wav(path: str) -> tuple[np.ndarray, int]:
    """只读 16-bit PCM（本仓测试音频都是这个格式），避免多装一个依赖。"""
    import wave

    with wave.open(path, "rb") as w:
        assert w.getsampwidth() == 2, "只支持 16-bit PCM WAV"
        channels = w.getnchannels()
        sr = w.getframerate()
        raw = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16)
    audio = raw.astype(np.float32) / 32768.0
    if channels > 1:
        audio = audio.reshape(-1, channels).mean(axis=1)
    return audio, sr


def parse_ids(path: str) -> list[int]:
    text = open(path).read().strip()
    body = text[text.index("[") + 1 : text.rindex("]")]
    return [int(x) for x in body.split(",") if x.strip()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("model_dir")
    ap.add_argument("wav")
    ap.add_argument("--dump", required=True, help="Rust 侧 XIME_ASR_DUMP 目录")
    ap.add_argument("--language", default="en")
    ap.add_argument("--task", default="transcribe")
    args = ap.parse_args()

    device = "cpu"
    processor = WhisperProcessor.from_pretrained(args.model_dir)
    model = (
        WhisperForConditionalGeneration.from_pretrained(args.model_dir, torch_dtype=torch.float32)
        .to(device)
        .eval()
    )

    ok = True

    # ---- 1) 前处理 ----
    audio, sr = read_wav(args.wav)
    assert sr == 16_000, f"参考实现要求 16 kHz，实际 {sr}"
    feats = processor.feature_extractor(
        audio, sampling_rate=sr, return_tensors="pt"
    ).input_features
    mel_ref = feats.numpy()[0]
    mel_got = read_bin(os.path.join(args.dump, "mel"))
    ok &= stats("前处理 log-mel", mel_ref, mel_got)

    # ---- 2) 编码器 ----
    with torch.no_grad():
        enc_ref = model.model.encoder(feats).last_hidden_state.numpy()[0]
    enc_got = read_bin(os.path.join(args.dump, "encoder"))
    ok &= stats("编码器输出", enc_ref, enc_got)

    # ---- 3) prompt ----
    prompt_path = os.path.join(args.dump, "prompt_ids.txt")
    rust_prompt = parse_ids(prompt_path) if os.path.exists(prompt_path) else []
    cfg = model.config
    gen = model.generation_config
    sot = cfg.decoder_start_token_id
    # transformers 5.x 的 `get_prompt_ids` 已经是「startofprev 文本提示」的语义，
    # 不再生成语言/task 前缀，这里按 generation_config 自己拼。
    lang_id = gen.lang_to_id[f"<|{args.language}|>"]
    task_id = gen.task_to_id[args.task]
    ref_prompt = [sot, lang_id, task_id, gen.no_timestamps_token_id]
    same_prompt = rust_prompt == ref_prompt
    ok &= same_prompt
    print(
        f"[{'ok' if same_prompt else 'FAIL'}] prompt: Rust {rust_prompt} / 参考 {ref_prompt}"
    )

    # ---- 4) 贪心解码文本 ----
    with torch.no_grad():
        generated = model.generate(
            feats,
            language=args.language,
            task=args.task,
            return_timestamps=False,
            do_sample=False,
            num_beams=1,
            max_new_tokens=200,
        )
    ref_text = processor.batch_decode(generated, skip_special_tokens=True)[0].strip()
    ids_path = os.path.join(args.dump, "ids.txt")
    if os.path.exists(ids_path):
        rust_ids = parse_ids(ids_path)
        # 只比正文：参考输出的序列含 prompt，掐掉公共前缀后比
        body = list(generated[0].tolist())
        if body[: len(ref_prompt)] == list(ref_prompt):
            body = body[len(ref_prompt) :]
        # 去掉 eot 与填充
        while body and body[-1] == cfg.eos_token_id:
            body.pop()
        ids_same = body == rust_ids
        ok &= ids_same
        print(
            f"[{'ok' if ids_same else 'FAIL'}] 贪心 token: Rust {len(rust_ids)} 个 / "
            f"参考 {len(body)} 个"
        )
        if not ids_same:
            print(f"  Rust: {rust_ids[:40]}")
            print(f"  参考: {body[:40]}")

    print(f"\n参考文本: {ref_text}")
    ids_path = os.path.join(args.dump, "ids.txt")
    if os.path.exists(ids_path):
        print(
            "Rust 文本: "
            + processor.tokenizer.decode(parse_ids(ids_path), skip_special_tokens=True).strip()
        )
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
