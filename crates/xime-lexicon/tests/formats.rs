//! 格式注册表：扩展名认领、`.txt` 内容探测、错误提示。

use std::path::Path;
use xime_lexicon::{
    CodeSystem, detect, find_by_file_name, formats, has_candidate, parse_bytes, parse_file,
    supported_description,
};

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture")
}

#[test]
fn resolves_unique_extensions_case_insensitively() {
    assert_eq!(find_by_file_name("a.scel").unwrap().id(), "sogou-scel");
    assert_eq!(find_by_file_name("A.SCEL").unwrap().id(), "sogou-scel");
    assert_eq!(
        find_by_file_name("汽车词汇大全.bdict").unwrap().id(),
        "baidu-bdict"
    );
    assert_eq!(find_by_file_name("A.BDICT").unwrap().id(), "baidu-bdict");
    assert_eq!(find_by_file_name("web-slang.dict").unwrap().id(), "libimebin");
    assert_eq!(find_by_file_name("搜狗备份.bin").unwrap().id(), "sgpybin");
}

#[test]
fn does_not_claim_rime_dictionary_or_unknown_files() {
    assert!(find_by_file_name("wubi86.dict.yaml").is_none());
    assert!(find_by_file_name("something.txt").is_none());
    assert!(!has_candidate("wubi86.dict.yaml"));
    assert!(has_candidate("words.txt"));
}

#[test]
fn txt_is_ambiguous_and_resolved_by_content() {
    assert_eq!(
        detect("words.txt", &fixture("libimetxt/libime.txt")).unwrap().id(),
        "libimetxt"
    );
    assert_eq!(
        detect("words.txt", &fixture("sgpy/sogou.txt")).unwrap().id(),
        "sgpy"
    );
    assert_eq!(
        detect("words.txt", &fixture("bdpy/baidu.txt")).unwrap().id(),
        "bdpy"
    );
    assert!(detect("words.txt", "hello world\n".as_bytes()).is_none());
}

#[test]
fn binary_formats_are_resolved_by_extension() {
    assert!(detect("web-slang.dict", "not a libime dict".as_bytes()).is_none());
    let error = parse_bytes("web-slang.dict", "not a libime dict".as_bytes()).unwrap_err();
    assert!(error.to_string().contains("magic"), "{error}");
}

#[test]
fn parse_bytes_handles_every_fixture() {
    let cases = [
        ("scel/errenzhuan.scel", "sogou-scel"),
        ("bdict/qiche.bdict", "baidu-bdict"),
        ("libimebin/web-slang.dict", "libimebin"),
        ("sgpybin/sogou-new.bin", "sgpybin"),
        ("sgpybin/sogou-old.bin", "sgpybin"),
        ("bdpy/baidu.txt", "bdpy"),
        ("libimetxt/libime.txt", "libimetxt"),
        ("sgpy/sogou.txt", "sgpy"),
    ];
    for (path, expected) in cases {
        let parsed = parse_bytes(path, &fixture(path)).expect(path);
        assert_eq!(parsed.format_id, expected, "{path}");
        assert_eq!(parsed.code_system, CodeSystem::Pinyin, "{path}");
        assert!(!parsed.entries.is_empty(), "{path}");
    }
}

#[test]
fn parse_file_reads_from_disk() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bdpy/baidu.txt");
    let parsed = parse_file(&path).expect("parse");
    assert_eq!(parsed.format_id, "bdpy");
    assert_eq!(parsed.entries.len(), 7);
}

#[test]
fn unsupported_extension_reports_supported_list() {
    let error = parse_bytes("something.xlsx", b"whatever").unwrap_err();
    assert!(error.to_string().contains("不支持"), "{error}");
    assert!(error.to_string().contains("搜狗细胞词库"), "{error}");
}

#[test]
fn empty_file_is_rejected() {
    let error = parse_bytes("a.scel", b"").unwrap_err();
    assert!(error.to_string().contains("空"), "{error}");
}

#[test]
fn description_lists_all_registered_formats() {
    let description = supported_description();
    for name in [
        "搜狗细胞词库",
        "百度分类词库",
        "Fcitx5/libime 文本词库",
        "搜狗拼音文本词库",
        "百度拼音文本词库",
        "Fcitx5/libime 二进制词库",
        "搜狗拼音备份词库",
    ] {
        assert!(description.contains(name), "缺少格式：{name}");
    }
    assert_eq!(formats().len(), 7);
}
