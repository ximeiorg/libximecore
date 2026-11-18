#![cfg(feature = "voice-page")]

// 决定性验证：Linux + voice-page 下侧栏是否含「语音输入」
#[test]
fn sidebar_contains_voice_entry() {
    let groups = xime_setup_lib::pages::sidebar_groups();
    for (name, items) in &groups {
        for (_icon, label) in items {
            println!("组[{name}] {label}");
        }
    }
    let flat: Vec<String> = groups
        .iter()
        .flat_map(|(_, items)| items.iter().map(|(_, l)| l.to_string()).collect::<Vec<_>>())
        .collect();
    assert!(
        flat.iter().any(|l| l == "语音输入"),
        "侧栏缺少语音输入：{flat:?}"
    );
}
