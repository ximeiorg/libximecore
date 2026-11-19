#!/usr/bin/env python3
"""把 ModelScope 上的 Fun-ASR-Nano-2512 原始 checkpoint 转成 xime-infer 能读的 safetensors。

原始仓库（`FunAudioLLM/Fun-ASR-Nano-2512`）只有 `model.pt`（PyTorch 序列化），
xime-infer 走 candle，只认 safetensors；而且两边的权重命名与结构都不一样，必须转：

| 原始（FunASR） | 转换后（transformers 布局） |
| --- | --- |
| `audio_encoder.encoders0.0.*` | `model.audio_tower.layers.0.*` |
| `audio_encoder.encoders.{i}.*` | `model.audio_tower.layers.{i+1}.*` |
| `audio_encoder.tp_encoders.{i}.*` | `model.audio_tower.layers.{50+i}.*` |
| `audio_encoder.after_norm.*` | `model.audio_tower.layers.49.final_layernorm.*` |
| `audio_encoder.tp_norm.*` | `model.audio_tower.layers.69.final_layernorm.*` |
| `audio_adaptor.linear{1,2}.*` | `model.multi_modal_projector.linear_{1,2}.*` |
| `audio_adaptor.blocks.{i}.*` | `model.multi_modal_projector.layers.{i}.*` |
| `llm.model.*` / `llm.lm_head.*` | `model.language_model.*` / `lm_head.*` |
| `self_attn.linear_q_k_v.*` | 按行切成 `self_attn.{q,k,v}_proj.*` |
| `self_attn.linear_out.*` | `self_attn.o_proj.*` |
| `self_attn.fsmn_block.*` | `self_attn.fsmn.conv.*` |
| `norm1/norm2` / `feed_forward.w_{1,2}` | `input_layernorm/post_attention_layernorm` / `mlp.fc{1,2}` |
| `ctc_decoder.*` / `ctc.*` | 丢弃（时间戳用的旁支，输入法不需要） |

映射与 transformers 官方的 `convert_fun_asr_nano_to_hf.py` 完全一致（已用真实
`model.pt` 核对过全部 1627 个键），区别只有三点：

1. **默认保持原始 dtype**：这份 checkpoint 的编码器与 LLM 全是 bf16，转出来仍是
   bf16；`--bf16` 是显式声明（当前与默认等价，留着是为了将来权重换精度）。
2. 顺带写出本项目需要的 `config.json` / `processor_config.json` 并复制分词器，
   产出目录可以直接丢进 `~/.config/xime/models/`。
3. `ctc_decoder.*` / `ctc.*`（时间戳旁支）与 `audio_encoder.tp_*`（时间戳预测塔）
   都不进模型，只保留前 50 层转写塔 + 后 20 层（也不丢弃，见下）。

> 注意：`audio_encoder.tp_encoders` 与 `tp_norm` 是**要留的**——transformers 布局里
> 它们就是第 50–69 层，`final_layernorm` 挂在第 69 层末尾。

用法：

```bash
# 1) 只装两个包（不需要 transformers / funasr）
python -m pip install torch safetensors

# 2) 下载原始 checkpoint（ModelScope，约 2.1 GB）
modelscope download --model FunAudioLLM/Fun-ASR-Nano-2512 --local_dir ./Fun-ASR-Nano-2512

# 3) 转换
python tools/convert_fun_asr_nano.py \\
    --model_path ./Fun-ASR-Nano-2512 \\
    --output_path ~/.config/xime/models/fun-asr-nano-2512
```

转换完可以用真实权重对拍：

```bash
XIME_FUNASR_MODEL_DIR=~/.config/xime/models/fun-asr-nano-2512 \\
XIME_FUNASR_WAV=path/to/zh.wav \\
  cargo test --release -p xime-infer --test fun_asr_parity -- --ignored --test-threads=1
```

`--self-test` 不需要权重，只校验键名映射与 QKV 切分逻辑：

```bash
python tools/convert_fun_asr_nano.py --self-test
```
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys

# --------------------------------------------------------------------------------------
# 键名映射
# --------------------------------------------------------------------------------------


def root_mapping(num_blocks: int, tp_blocks: int):
    """前缀映射：FunASR 顶层模块 → transformers 布局。"""
    return (
        (r"^audio_encoder\.encoders0\.0\.", "model.audio_tower.layers.0."),
        (
            r"^audio_encoder\.encoders\.(\d+)\.",
            lambda m: f"model.audio_tower.layers.{int(m[1]) + 1}.",
        ),
        (
            r"^audio_encoder\.tp_encoders\.(\d+)\.",
            lambda m: f"model.audio_tower.layers.{int(m[1]) + num_blocks}.",
        ),
        (
            r"^audio_encoder\.after_norm\.",
            f"model.audio_tower.layers.{num_blocks - 1}.final_layernorm.",
        ),
        (
            r"^audio_encoder\.tp_norm\.",
            f"model.audio_tower.layers.{num_blocks + tp_blocks - 1}.final_layernorm.",
        ),
        (r"^audio_adaptor\.blocks\.", "model.multi_modal_projector.layers."),
        (r"^audio_adaptor\.linear1\.", "model.multi_modal_projector.linear_1."),
        (r"^audio_adaptor\.linear2\.", "model.multi_modal_projector.linear_2."),
        (r"^llm\.lm_head\.", "lm_head."),
        (r"^llm\.model\.", "model.language_model."),
    )


#: 组件内部的重命名（编码器层、adaptor 层通用）。
COMPONENT_MAPPING = (
    (r"\.feed_forward\.w_1\.", ".mlp.fc1."),
    (r"\.feed_forward\.w_2\.", ".mlp.fc2."),
    (r"\.norm1\.", ".input_layernorm."),
    (r"\.norm2\.", ".post_attention_layernorm."),
    (r"\.self_attn\.linear_q\.", ".self_attn.q_proj."),
    (r"\.self_attn\.linear_k\.", ".self_attn.k_proj."),
    (r"\.self_attn\.linear_v\.", ".self_attn.v_proj."),
    (r"\.self_attn\.linear_out\.", ".self_attn.o_proj."),
    (r"\.self_attn\.fsmn_block\.", ".self_attn.fsmn.conv."),
)

#: 明确丢弃的旁支（CTC 时间戳）。
#:
#: `audio_encoder.tp_*` **不在此列**：它是时间戳预测塔，但 transformers 布局把它
#: 当作第 50–69 层一起用，丢了形状就对不上。
DROPPED_PREFIXES = ("ctc_decoder.", "ctc.")


def convert_key(key: str, num_blocks: int = 50, tp_blocks: int = 20) -> str | None:
    """单个键 → 转换后的键；返回 None 表示该键不进模型（CTC 旁支等）。"""
    mapped = None
    for pattern, replacement in root_mapping(num_blocks, tp_blocks):
        if re.match(pattern, key):
            mapped = re.sub(pattern, replacement, key)
            break
    if mapped is None:
        return None
    for pattern, replacement in COMPONENT_MAPPING:
        mapped = re.sub(pattern, replacement, mapped)
    return mapped


def split_fused_qkv(tensor, name: str, num_blocks: int = 50, tp_blocks: int = 20):
    """`linear_q_k_v` 是按行拼起来的 q/k/v，切成三份。"""
    if tensor.shape[0] % 3 != 0:
        raise SystemExit(f"融合 QKV 的第一维不能被 3 整除：{name} {tuple(tensor.shape)}")
    parts = tensor.chunk(3, dim=0)
    out = {}
    for suffix, part in zip(("q", "k", "v"), parts):
        target = convert_key(name.replace("linear_q_k_v", f"{suffix}_proj"), num_blocks, tp_blocks)
        if target is None:
            raise SystemExit(f"切分后的键无法映射：{name}")
        out[target] = part
    return out


# --------------------------------------------------------------------------------------
# config.yaml 解析（只支持 FunASR 那种两层、标量取值的子集）
# --------------------------------------------------------------------------------------

_SCALAR = re.compile(r"^(?P<indent>\s*)(?P<key>[A-Za-z0-9_\-]+):\s*(?P<value>.*)$")


def parse_simple_yaml(path: str) -> dict:
    """解析 `key:\\n  sub: value` 两层结构；列表、多行块直接忽略。

    不引 PyYAML 是为了让脚本只依赖 torch + safetensors。FunASR 的 config.yaml
    恰好只有这种形状（列表只在 `target_modules` 这类用不到的字段上）。
    """
    root: dict[str, dict] = {}
    current = None
    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            match = _SCALAR.match(line)
            if not match:
                continue
            indent = len(match.group("indent"))
            key, value = match.group("key"), match.group("value").strip()
            if indent == 0:
                current = root.setdefault(key, {})
                if value:
                    current["__value__"] = value
            elif current is not None:
                current[key] = value
    return root


def scalar(section: dict, key: str, default):
    raw = section.get(key)
    if raw is None or raw == "":
        return default
    raw = raw.strip().strip("'\"")
    try:
        if "." in raw or "e" in raw.lower():
            return float(raw)
        return int(raw)
    except ValueError:
        return default


# --------------------------------------------------------------------------------------
# config.json 构造
# --------------------------------------------------------------------------------------


def build_configs(yaml_cfg: dict, qwen_cfg: dict) -> tuple[dict, dict]:
    """由 config.yaml + Qwen3 的 config.json 生成模型配置与 processor 配置。"""
    enc = yaml_cfg.get("audio_encoder_conf", {})
    adaptor = yaml_cfg.get("audio_adaptor_conf", {})
    frontend = yaml_cfg.get("frontend_conf", {})

    num_blocks = scalar(enc, "num_blocks", 50)
    tp_blocks = scalar(enc, "tp_blocks", 20)
    lfr_m = scalar(frontend, "lfr_m", 7)
    lfr_n = scalar(frontend, "lfr_n", 6)
    n_mels = scalar(frontend, "n_mels", 80)

    llm_dim = scalar(adaptor, "llm_dim", qwen_cfg.get("hidden_size", 1024))

    config = {
        "architectures": ["FunAsrNanoForConditionalGeneration"],
        "model_type": "fun_asr_nano",
        "audio_token_id": 151646,
        "tie_word_embeddings": True,
        "audio_config": {
            "num_mel_bins": n_mels,
            "num_stacked_frames": lfr_m,
            "hidden_size": scalar(enc, "output_size", 512),
            "num_attention_heads": scalar(enc, "attention_heads", 4),
            "intermediate_size": scalar(enc, "linear_units", 2048),
            "num_hidden_layers": num_blocks + tp_blocks,
            "num_timestamp_prediction_layers": tp_blocks,
            "layer_norm_eps": 1e-5,
            "fsmn_kernel_size": scalar(enc, "kernel_size", 11),
            "max_position_embeddings": 2049,
            "hidden_act": "relu",
        },
        # heads / FFN 取值来自 transformers 的 FunAsrNanoAdaptorConfig 默认值：
        # FunASR 的 audio_adaptor_conf 只给了 encoder_dim / llm_dim / ffn_dim / n_layer，
        # 其中 ffn_dim=2048 对应投影层的隐层（projector_hidden_size），不是 block 的 FFN。
        "adaptor_config": {
            "hidden_size": llm_dim,
            "num_attention_heads": 8,
            "intermediate_size": llm_dim // 4,
            "num_hidden_layers": scalar(adaptor, "n_layer", 2),
            "layer_norm_eps": 1e-5,
            "projector_hidden_size": scalar(adaptor, "ffn_dim", 2048),
            "hidden_act": "relu",
            "projector_hidden_act": "relu",
        },
        "text_config": qwen_cfg,
    }

    processor = {
        "audio_token": "<|object_ref_start|>",
        "processor_class": "FunAsrNanoProcessor",
        "feature_extractor": {
            "feature_extractor_type": "FunAsrNanoFeatureExtractor",
            "feature_size": n_mels,
            "sampling_rate": scalar(frontend, "fs", 16000),
            "frame_length": scalar(frontend, "frame_length", 25),
            "frame_shift": scalar(frontend, "frame_shift", 10),
            "num_frames_lfr": lfr_m,
            "stride_lfr": lfr_n,
            "padding_side": "right",
            "padding_value": 0.0,
            "return_attention_mask": True,
        },
    }
    return config, processor


# --------------------------------------------------------------------------------------
# 主流程
# --------------------------------------------------------------------------------------


def load_state_dict(path: str) -> dict:
    import torch

    print(f"装载 {path} …（约 2 GB，需要一点时间）")
    try:
        state = torch.load(path, map_location="cpu", weights_only=True)
    except Exception:  # noqa: BLE001 - 老 pickle 需要关闭 weights_only
        print("  weights_only=True 失败，改用完整反序列化（仅适用于可信来源）")
        state = torch.load(path, map_location="cpu", weights_only=False)
    for wrapper in ("state_dict", "model_state_dict", "model"):
        if isinstance(state.get(wrapper), dict):
            state = state[wrapper]
            break
    return {
        (k[len("module.") :] if k.startswith("module.") else k): v
        for k, v in state.items()
    }


def load_qwen_llm(qwen_dir: str) -> dict:
    """LLM 权重不在 `model.pt` 里时，从子目录的 safetensors 补上。"""
    from safetensors.torch import load_file

    index = os.path.join(qwen_dir, "model.safetensors.index.json")
    if os.path.exists(index):
        with open(index, encoding="utf-8") as f:
            weight_map = json.load(f)["weight_map"]
        files = sorted({os.path.join(qwen_dir, name) for name in weight_map.values()})
    else:
        single = os.path.join(qwen_dir, "model.safetensors")
        if not os.path.exists(single):
            raise SystemExit(f"找不到 LLM 权重：{qwen_dir}（model.safetensors 也不存在）")
        files = [single]

    out = {}
    for path in files:
        for key, value in load_file(path).items():
            out[f"llm.{key}"] = value
    return out


def convert(model_path: str, output_path: str, keep_dtype: bool, skip_verify: bool):
    import torch
    from safetensors.torch import save_file

    config_yaml = os.path.join(model_path, "config.yaml")
    checkpoint = os.path.join(model_path, "model.pt")
    qwen_dir = os.path.join(model_path, "Qwen3-0.6B")
    if not os.path.exists(config_yaml):
        raise SystemExit(f"缺少 {config_yaml}")
    if not os.path.exists(checkpoint):
        raise SystemExit(f"缺少 {checkpoint}")

    yaml_cfg = parse_simple_yaml(config_yaml)
    enc = yaml_cfg.get("audio_encoder_conf", {})
    num_blocks = scalar(enc, "num_blocks", 50)
    tp_blocks = scalar(enc, "tp_blocks", 20)

    qwen_config_path = os.path.join(qwen_dir, "config.json")
    if not os.path.exists(qwen_config_path):
        raise SystemExit(f"缺少 {qwen_config_path}（原始仓库里应有 Qwen3-0.6B/ 子目录）")
    with open(qwen_config_path, encoding="utf-8") as f:
        qwen_cfg = json.load(f)

    state = load_state_dict(checkpoint)

    # LLM 权重可能已被训练脚本排除在 model.pt 之外（`effective_save_name_excludes: llm.`）。
    if not any(k.startswith("llm.") for k in state):
        print("model.pt 里没有 llm.*，从 Qwen3-0.6B/ 补齐")
        state.update(load_qwen_llm(qwen_dir))

    converted: dict[str, torch.Tensor] = {}
    dropped: list[str] = []
    for key, value in state.items():
        if ".self_attn.linear_q_k_v." in key:
            converted.update(split_fused_qkv(value, key, num_blocks, tp_blocks))
            continue
        target = convert_key(key, num_blocks, tp_blocks)
        if target is None:
            dropped.append(key)
            continue
        # 只有 f32 的浮点权重需要保留原精度；bf16/f16 原样带走。
        if not keep_dtype and value.is_floating_point():
            value = value.to(torch.bfloat16)
        converted[target] = value.contiguous()

    # CTC 旁支属于预期丢弃，其余没映射上的键要让人看见。
    unexpected = [k for k in dropped if not k.startswith(DROPPED_PREFIXES)]
    print(f"转换 {len(converted)} 个张量，丢弃 {len(dropped)} 个（其中 CTC 旁支 "
          f"{len(dropped) - len(unexpected)} 个）")
    for key in unexpected[:10]:
        print(f"  ! 未映射：{key}")
    if len(unexpected) > 10:
        print(f"  ! …另有 {len(unexpected) - 10} 个")

    os.makedirs(output_path, exist_ok=True)
    weights_path = os.path.join(output_path, "model.safetensors")
    print(f"写出 {weights_path} …")
    save_file(converted, weights_path, metadata={"format": "pt"})

    config, processor = build_configs(yaml_cfg, qwen_cfg)
    # 记录**真实**写出的精度，别猜。本实现不读这个字段，但人要看得懂目录里是什么。
    dtypes = sorted(
        {str(t.dtype).rsplit(".", 1)[-1] for t in converted.values() if t.is_floating_point()}
    )
    config["dtype"] = dtypes[0] if len(dtypes) == 1 else "mixed:" + ",".join(dtypes)
    with open(os.path.join(output_path, "config.json"), "w", encoding="utf-8") as f:
        json.dump(config, f, ensure_ascii=False, indent=2)
    with open(os.path.join(output_path, "processor_config.json"), "w", encoding="utf-8") as f:
        json.dump(processor, f, ensure_ascii=False, indent=2)

    # 分词器：tokenizer.json 是必需项，其余有就一起带上。
    copied = []
    for name in (
        "tokenizer.json",
        "tokenizer_config.json",
        "vocab.json",
        "merges.txt",
        "generation_config.json",
    ):
        src = os.path.join(qwen_dir, name)
        if os.path.exists(src):
            shutil.copy2(src, os.path.join(output_path, name))
            copied.append(name)
    if "tokenizer.json" not in copied:
        raise SystemExit(
            "Qwen3-0.6B/ 里没有 tokenizer.json；xime-infer 需要它（BPE + byte-level）"
        )

    # 模型目录的展示信息（宿主读 model.json 显示名称与说明）。
    meta = {
        "name": "Fun-ASR-Nano 2512",
        "description": "通义 Fun-ASR-Nano（0.8B）：中英日 + 7 种方言 / 26 种口音，整段识别",
    }
    with open(os.path.join(output_path, "model.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, ensure_ascii=False, indent=2)

    if not skip_verify:
        verify(converted, config)
    print(f"\n完成：{output_path}")
    print("把这个目录放进 ~/.config/xime/models/ 后在设置页选中即可。")


def verify(tensors: dict, config: dict) -> None:
    """按 config 抽查关键张量的形状，挡住「映射对但结构不对」的情况。"""
    audio = config["audio_config"]
    adaptor = config["adaptor_config"]
    text = config["text_config"]
    layers = audio["num_hidden_layers"]
    checks = {
        "model.audio_tower.layers.0.input_layernorm.weight": (
            audio["num_mel_bins"] * audio["num_stacked_frames"],
        ),
        "model.audio_tower.layers.1.self_attn.q_proj.weight": (
            audio["hidden_size"],
            audio["hidden_size"],
        ),
        "model.audio_tower.layers.1.self_attn.fsmn.conv.weight": (
            audio["hidden_size"],
            1,
            audio["fsmn_kernel_size"],
        ),
        f"model.audio_tower.layers.{layers - 1}.final_layernorm.weight": (audio["hidden_size"],),
        "model.multi_modal_projector.linear_1.weight": (
            adaptor["projector_hidden_size"],
            audio["hidden_size"],
        ),
        "model.multi_modal_projector.linear_2.weight": (
            adaptor["hidden_size"],
            adaptor["projector_hidden_size"],
        ),
        "model.language_model.embed_tokens.weight": (text["vocab_size"], text["hidden_size"]),
    }
    problems = []
    for name, shape in checks.items():
        got = tensors.get(name)
        if got is None:
            problems.append(f"缺 {name}")
        elif tuple(got.shape) != shape:
            problems.append(f"{name} 形状 {tuple(got.shape)}，期望 {shape}")
    if problems:
        raise SystemExit("校验失败：\n  " + "\n  ".join(problems))
    params = sum(t.numel() for t in tensors.values())
    print(f"校验通过：{len(tensors)} 个张量，约 {params / 1e6:.0f}M 参数")


# --------------------------------------------------------------------------------------
# 自检（不需要权重）
# --------------------------------------------------------------------------------------


def self_test() -> int:
    failures = []

    def expect(actual, expected, label):
        if actual != expected:
            failures.append(f"{label}: 得到 {actual!r}，期望 {expected!r}")

    cases = {
        "audio_encoder.encoders0.0.norm1.weight": "model.audio_tower.layers.0.input_layernorm.weight",
        "audio_encoder.encoders.0.feed_forward.w_1.weight": "model.audio_tower.layers.1.mlp.fc1.weight",
        "audio_encoder.encoders.48.self_attn.linear_out.bias": "model.audio_tower.layers.49.self_attn.o_proj.bias",
        "audio_encoder.tp_encoders.0.norm2.weight": "model.audio_tower.layers.50.post_attention_layernorm.weight",
        "audio_encoder.tp_encoders.19.self_attn.fsmn_block.weight": "model.audio_tower.layers.69.self_attn.fsmn.conv.weight",
        "audio_encoder.after_norm.weight": "model.audio_tower.layers.49.final_layernorm.weight",
        "audio_encoder.tp_norm.bias": "model.audio_tower.layers.69.final_layernorm.bias",
        "audio_adaptor.linear1.weight": "model.multi_modal_projector.linear_1.weight",
        "audio_adaptor.linear2.weight": "model.multi_modal_projector.linear_2.weight",
        "audio_adaptor.blocks.1.self_attn.linear_q.bias": "model.multi_modal_projector.layers.1.self_attn.q_proj.bias",
        "audio_adaptor.blocks.0.feed_forward.w_2.weight": "model.multi_modal_projector.layers.0.mlp.fc2.weight",
        "llm.model.layers.0.self_attn.q_proj.weight": "model.language_model.layers.0.self_attn.q_proj.weight",
        "llm.lm_head.weight": "lm_head.weight",
        "ctc_decoder.encoders.0.norm1.weight": None,
        "some.unknown.tensor": None,
    }
    for source, expected in cases.items():
        expect(convert_key(source), expected, source)

    # 融合 QKV：模拟 [1536, 560] 的 q/k/v 拼接
    try:
        import torch

        fused = torch.arange(6 * 4, dtype=torch.float32).reshape(6, 4)
        parts = split_fused_qkv(fused, "audio_encoder.encoders0.0.self_attn.linear_q_k_v.weight")
        expect(
            sorted(parts),
            [
                "model.audio_tower.layers.0.self_attn.k_proj.weight",
                "model.audio_tower.layers.0.self_attn.q_proj.weight",
                "model.audio_tower.layers.0.self_attn.v_proj.weight",
            ],
            "QKV 切分的键名",
        )
        expect(tuple(parts["model.audio_tower.layers.0.self_attn.q_proj.weight"].shape), (2, 4), "QKV 切分形状")
        expect(
            parts["model.audio_tower.layers.0.self_attn.k_proj.weight"].tolist()[0],
            [8.0, 9.0, 10.0, 11.0],
            "QKV 切分内容",
        )
    except ImportError:
        print("（未安装 torch，跳过 QKV 切分自检）")

    # 简易 YAML 解析
    sample = "audio_encoder_conf:\n  output_size: 512\n  tp_blocks: 20\nllm_conf:\n  init_param_path: Qwen3-0.6B\n"
    tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".self-test.yaml")
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(sample)
    try:
        cfg = parse_simple_yaml(tmp)
        expect(scalar(cfg["audio_encoder_conf"], "output_size", 0), 512, "yaml output_size")
        expect(cfg["llm_conf"]["init_param_path"], "Qwen3-0.6B", "yaml init_param_path")
    finally:
        os.remove(tmp)

    if failures:
        print("自检失败：")
        for line in failures:
            print("  -", line)
        return 1
    print(f"自检通过（{len(cases)} 个键名映射 + QKV 切分 + YAML 解析）")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="把 Fun-ASR-Nano-2512 的 model.pt 转成 xime-infer 用的 safetensors"
    )
    parser.add_argument("--model_path", help="原始模型目录（含 config.yaml / model.pt / Qwen3-0.6B）")
    parser.add_argument("--output_path", help="输出目录")
    parser.add_argument(
        "--bf16",
        action="store_true",
        help="把浮点权重一律压成 bf16（这份 checkpoint 本来就是 bf16，默认即等价）",
    )
    parser.add_argument("--skip-verify", action="store_true", help="跳过转换后的形状校验")
    parser.add_argument("--self-test", action="store_true", help="只跑键名映射自检，不需要权重")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if not args.model_path or not args.output_path:
        parser.error("需要 --model_path 与 --output_path（或 --self-test）")

    convert(args.model_path, args.output_path, keep_dtype=not args.bf16, skip_verify=args.skip_verify)
    return 0


if __name__ == "__main__":
    sys.exit(main())
