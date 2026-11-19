"""用 transformers 的 FunAsrNano 参考实现给 Rust 侧做数值对拍。

三段分别比，错在哪一层能直接看出来：

1. 前处理（Kaldi fbank + LFR）—— 与 `FunAsrNanoFeatureExtractor` 的输出比；
2. 编码器 + 投影 —— 与 `model.model.get_audio_features` 的输出比；
3. 解码 —— 用**同一串 prompt token** 调 `generate`，比最终文本。

用法：

    # 先在 Rust 侧导出中间量
    XIME_ASR_DUMP=/tmp/funasr_dump cargo run --release -p xime-infer \\
        --example fun_asr_transcribe -- <模型目录> <wav> 中文

    python tools/fun_asr_reference.py <模型目录> <wav> --dump /tmp/funasr_dump
"""

from __future__ import annotations

import argparse
import json
import os
import sys

import numpy as np
import torch

from transformers import AutoProcessor, FunAsrNanoForConditionalGeneration


def read_bin(path: str, shape: str) -> np.ndarray:
    dims = tuple(int(x) for x in open(shape).read().split())
    data = np.fromfile(path, dtype=np.float32)
    return data.reshape(dims)


def stats(name: str, ref: np.ndarray, got: np.ndarray, tol: float = 1e-3) -> bool:
    """打印差值分布；判定用**相对 RMS**（与量纲无关），不看最大绝对值。

    对数特征在能量接近 `f32::EPSILON`（log ≈ -15.9）时会剧烈放大 1e-8 级的
    舍入差；编码器输出又是 LayerNorm 后的量级，绝对差值随数值尺度变化。所以
    统一用 `‖ref-got‖ / ‖ref‖`，并额外报出「差值 > 0.05 的点数」便于排查。

    `tol=1e-3` 对应 bf16 权重的舍入量级（约 8 bit 尾数 → 4e-3 相对）。
    """
    if ref.shape != got.shape:
        print(f"  {name}: 形状不同 ref{ref.shape} vs rust{got.shape}  ✗")
        return False
    diff = np.abs(ref - got)
    cos = float(
        (ref.ravel() @ got.ravel())
        / (np.linalg.norm(ref.ravel()) * np.linalg.norm(got.ravel()) + 1e-12)
    )
    rel_rms = float(np.linalg.norm(diff.ravel()) / (np.linalg.norm(ref.ravel()) + 1e-12))
    outliers = int((diff > 0.05).sum())
    ok = cos > 0.9999 and rel_rms < tol
    print(
        f"  {name}: 形状 {ref.shape}  中位差 {np.median(diff):.2e}  "
        f"相对RMS {rel_rms:.2e}  最大 {diff.max():.2e}  余弦 {cos:.6f}  "
        f">0.05 的点 {outliers}/{diff.size}  {'✓' if ok else '✗'}"
    )
    return ok


def load_wav(path: str) -> np.ndarray:
    """读 PCM16 WAV → float32 mono（不走 torchaudio，避免它索要 torchcodec）。"""
    import wave as pywave

    with pywave.open(path, "rb") as f:
        assert f.getsampwidth() == 2, "只支持 PCM16"
        channels, rate = f.getnchannels(), f.getframerate()
        raw = f.readframes(f.getnframes())
    data = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0
    if channels > 1:
        data = data.reshape(-1, channels).mean(1)
    if rate != 16000:
        raise SystemExit(f"采样率 {rate}，请先转成 16 kHz")
    return data


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("model_dir")
    ap.add_argument("wav")
    ap.add_argument("--dump", default="/tmp/funasr_dump", help="Rust 侧导出的中间量目录")
    ap.add_argument("--ids", default=None, help="prompt token 文件（默认读 <dump>/prompt_ids.txt）")
    args = ap.parse_args()

    ok = True
    processor = AutoProcessor.from_pretrained(args.model_dir)
    model = FunAsrNanoForConditionalGeneration.from_pretrained(
        args.model_dir, dtype=torch.float32
    ).eval()

    audio = load_wav(args.wav)

    # ---- 1) 前处理 ----
    feats = processor.feature_extractor(
        audio, sampling_rate=16000, return_attention_mask=True, return_tensors="pt"
    )
    ref_feats = feats["input_features"]
    rust_feats = read_bin(f"{args.dump}/feats_f32.bin", f"{args.dump}/feats_shape.txt")
    ok &= stats("前处理 feats", ref_feats[0].numpy(), rust_feats, tol=5e-3)

    # ---- 2) 编码器 + 投影 ----
    with torch.inference_mode():
        ref_audio = model.model.get_audio_features(
            ref_feats, feats["input_features_mask"]
        ).pooler_output
    rust_audio = read_bin(f"{args.dump}/audio_f32.bin", f"{args.dump}/audio_shape.txt")
    ok &= stats("编码器 audio", ref_audio.numpy(), rust_audio)

    # ---- 3) 解码（用 Rust 导出的同一串 prompt id）----
    ids_path = args.ids or f"{args.dump}/prompt_ids.txt"
    ids = json.loads(open(ids_path).read())
    input_ids = torch.tensor([ids], dtype=torch.long)
    # 音频 embedding 拼在 user 指令之后：prompt_ids.txt 里 prefix 与 suffix 被直接
    # 接在一起，所以按 `suffix = <|im_end|>\n<|im_start|>assistant\n`（5 个 token）切开。
    split_at = len(ids) - 5
    with torch.inference_mode():
        text_embeds = model.model.get_input_embeddings()(input_ids)
        embeds = torch.cat(
            [
                text_embeds[:, :split_at],
                ref_audio.unsqueeze(0).to(text_embeds.dtype),
                text_embeds[:, split_at:],
            ],
            dim=1,
        )
        out = model.generate(
            inputs_embeds=embeds,
            attention_mask=torch.ones(embeds.shape[:2], dtype=torch.long),
            max_new_tokens=256,
            do_sample=False,
        )
    ids_ref = out[0].tolist()
    text = processor.batch_decode(out, skip_special_tokens=True)[0]
    print(f"  参考文本: {text!r}")
    print(f"  token ids: {ids_ref}")

    # Rust 侧生成的 id（示例里用 XIME_ASR_DUMP_IDS 导出）：除结尾的 eos 外应完全一致
    rust_ids_path = f"{args.dump}/ids.txt"
    if os.path.exists(rust_ids_path):
        rust_ids = json.loads(open(rust_ids_path).read())
        same = ids_ref[: len(rust_ids)] == rust_ids
        print(
            f"  Rust token ids: {rust_ids}\n"
            f"  与参考一致（前 {len(rust_ids)} 个）: {'✓' if same else '✗'}"
        )
        ok &= same

    print("结论:", "全部通过" if ok else "存在差异")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
