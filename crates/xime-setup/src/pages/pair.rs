#![cfg(feature = "pair-page")]
//! 设备关联页：局域网 P2P 配对。
//!
//! 两个配对方向用标签栏分开，一次只显示一个：
//! - 本机显示 6 位码，对方输入（显示端）
//! - 输入对方显示的码（输入端）
//!
//! 并排摆放会让页面上同时出现两个主按钮（「生成动态码」和「配对」），用户
//! 无法判断该点哪个；而且码显示出来以后，下方的输入框和它挤在一起，容易
//! 被当成「在这里填上面那组码」。
//!
//! 设计约束（见 `docs/clipboard-p2p-protocol.md`）：码只经人眼传递，不进报文；
//! 全程无服务器。界面上不出现 IP 输入框，除非用户主动展开「手动地址」——
//! 自动发现覆盖多数情况，兜底入口只是给 AP 隔离 / 广播被拦的场景留条路。

use crate::components::settings::{settings_group, settings_item, settings_page};
use crate::components::widgets::{
    badge, button_disabled, button_primary, button_secondary, label, medium, progress, semibold,
    tab_bar, text_button, text_input_style,
};
use crate::state::{Message, P2pPeerState, PairState, PairTab, SettingsState};
use crate::theme::ThemeColors;
use iced::widget::{column, container, row, text, text_input, Space};
use iced::{border, Alignment, Background, Element, Length};

/// 标签栏文案。顺序与 [`PairTab::index`] 一致。
const PAIR_TABS: [&str; 2] = ["显示配对码", "输入配对码"];

pub fn view<'a>(settings: &'a SettingsState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let pair = &settings.pair;
    let mut groups: Vec<Element<'a, Message>> = Vec::new();

    if let Some((msg, is_error)) = &pair.message {
        groups.push(status_banner(msg, *is_error, colors));
    }
    groups.push(local_group(pair, colors));
    groups.push(pair_group(pair, colors));
    groups.push(peers_group(pair, colors));

    settings_page("设备关联", colors, groups)
}

/// 顶部状态条：成功用绿、失败用红，不抢标题的视觉权重。
fn status_banner<'a>(msg: &'a str, is_error: bool, colors: &ThemeColors) -> Element<'a, Message> {
    let c = *colors;
    let accent = if is_error { c.error } else { c.success };
    container(text(msg).size(13).color(accent))
        .width(Length::Fill)
        .padding([10, 14])
        .style(move |_| container::Style {
            background: Some(Background::Color(accent.scale_alpha(0.10))),
            border: border::Border {
                color: accent.scale_alpha(0.35),
                width: 1.0,
                radius: border::radius(8.0),
            },
            ..container::Style::default()
        })
        .into()
}

fn local_group<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let name_input = text_input("设备名", &pair.device_name).size(14).padding([8, 10])
        .on_input(Message::PairDeviceNameChanged)
        .style(move |_t, status| text_input_style(colors, status))
        .width(200);

    // 标识由本机密钥派生：用户看不出内容，但换了身份一眼能发现（排障用）。
    let did_text = if pair.did.is_empty() {
        "未生成".to_string()
    } else {
        pair.did_short()
    };
    let did_label: Element<'a, Message> = text(did_text)
        .size(13)
        .font(medium())
        .color(colors.foreground_muted)
        .into();

    settings_group(
        "本机",
        Some("设备名只用来让对方认出这台机器，随时可以改"),
        colors,
        vec![
            settings_item(
                "设备名",
                Some("建议写清是哪台机器，例如「办公室 Mac」"),
                colors,
                row![
                    name_input,
                    button_secondary("保存", colors, Message::PairDeviceNameSave),
                ]
                .spacing(8)
                .into(),
            ),
            settings_item(
                "设备标识",
                Some("本机的固定标识，两台设备可对照确认是同一台机器"),
                colors,
                did_label,
            ),
        ],
    )
}

fn pair_group<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> =
        vec![tab_bar(&PAIR_TABS, pair.tab.index(), colors, Message::PairTab)];

    // 另一个方向还在跑时，当前 tab 里看不到它的进度。不提示的话用户会以为
    // 点击没生效，反复点，既占端口又让人更慌。
    if let Some(hint) = cross_tab_hint(pair, colors) {
        items.push(hint);
    }

    items.push(match pair.tab {
        PairTab::Display => display_block(pair, colors),
        PairTab::Join => join_block(pair, colors),
    });

    settings_group(
        "配对",
        Some("两台设备连同一个 Wi-Fi：一台显示 6 位数字，另一台输入即可。全程不经过任何服务器"),
        colors,
        items,
    )
}

/// 另一方向的进行状态（显示在当前 tab 顶部）。
fn cross_tab_hint<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Option<Element<'a, Message>> {
    let line = match pair.tab {
        PairTab::Join if pair.display.is_some() => {
            let left = pair.display_remaining().unwrap_or(0);
            format!("本机正在等待配对，配对码还剩 {left} 秒 —— 切到「显示配对码」可查看")
        }
        PairTab::Display if pair.joining => {
            "正在查找对方设备，结果会显示在页面顶部".to_string()
        }
        _ => return None,
    };
    Some(info_line(line, colors))
}

/// 中性提示条：用主色而不是错误色——这是「正在进行」，不是出了问题。
fn info_line<'a>(msg: String, colors: &ThemeColors) -> Element<'a, Message> {
    let c = *colors;
    container(text(msg).size(13).color(c.primary))
        .width(Length::Fill)
        .padding([8, 12])
        .style(move |_| container::Style {
            background: Some(Background::Color(c.primary.scale_alpha(0.08))),
            border: border::Border {
                color: c.primary.scale_alpha(0.25),
                width: 1.0,
                radius: border::radius(8.0),
            },
            ..container::Style::default()
        })
        .into()
}

/// 显示端：未生成时是一行入口，生成后是码 + 倒计时。
fn display_block<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let Some(info) = &pair.display else {
        return row![
            column![
                text("由这台设备显示配对码")
                    .size(14)
                    .font(medium())
                    .color(colors.foreground),
                text("生成后 60 秒内，在另一台设备上输入")
                    .size(13)
                    .color(colors.foreground_muted),
            ]
            .spacing(4)
            .width(Length::Fill),
            button_primary("生成配对码", colors, Message::PairShowCode),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into();
    };

    let remaining = pair.display_remaining().unwrap_or(0);
    let expired = remaining == 0;
    let countdown: Element<'a, Message> = if expired {
        text("已过期，请重新生成")
            .size(13)
            .color(colors.error)
            .into()
    } else {
        text(format!("剩余 {remaining} 秒"))
            .size(13)
            .color(colors.foreground_muted)
            .into()
    };

    column![
        text("在另一台设备上输入这组码")
            .size(13)
            .color(colors.foreground_muted),
        text(&info.code)
            .size(34)
            .font(semibold())
            .color(colors.primary),
        progress(pair.display_progress(), colors),
        row![
            countdown,
            Space::new().width(Length::Fill),
            button_secondary("换一个", colors, Message::PairShowCode),
            button_secondary("取消", colors, Message::PairCancelCode),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(10)
    .into()
}

/// 输入端：码输入框 + 配对按钮（+ 手填地址兜底）。
fn join_block<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let input = text_input("例如 482 719", &pair.code_input).size(14).padding([8, 10])
        .on_input(Message::PairCodeInputChanged)
        .style(move |_t, status| text_input_style(colors, status))
        .width(160);

    let action: Element<'a, Message> = if pair.joining {
        button_disabled("正在查找设备…", colors)
    } else {
        button_primary("配对", colors, Message::PairJoin).into()
    };

    let mut block = column![
        text("在这台设备上输入配对码")
            .size(14)
            .font(medium())
            .color(colors.foreground),
        row![input, action].spacing(8).align_y(Alignment::Center),
        row![
            text("填另一台设备上显示的 6 位数字，会自动在同一 Wi-Fi 下找到对方，最长等 60 秒")
                .size(12)
                .color(colors.foreground_faint),
            Space::new().width(Length::Fill),
            text_button(
                if pair.show_manual {
                    "收起手动地址"
                } else {
                    "手动地址"
                },
                colors,
                Message::PairManualToggle,
            ),
        ]
        .align_y(Alignment::Center),
    ]
    .spacing(8);

    if pair.show_manual {
        block = block.push(
            row![
                text_input("192.168.1.5 或 192.168.1.5:47823", &pair.addr_input).size(14).padding([8, 10])
                    .on_input(Message::PairAddrInputChanged)
                    .style(move |_t, status| text_input_style(colors, status))
                    .width(260),
                text("填另一台设备在网络中的地址")
                    .size(12)
                    .color(colors.foreground_faint),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    block.into()
}

fn peers_group<'a>(pair: &'a PairState, colors: &'a ThemeColors) -> Element<'a, Message> {
    let mut items: Vec<Element<'a, Message>> = Vec::new();

    if let Some(err) = &pair.peer_error {
        items.push(
            text(format!("配对表读取失败：{err}"))
                .size(13)
                .color(colors.error)
                .into(),
        );
    } else if pair.peers.is_empty() {
        items.push(settings_item(
            "暂无已配对设备",
            Some("完成一次配对后，设备会出现在这里；移除后需要重新配对"),
            colors,
            label("", colors),
        ));
    }

    for peer in &pair.peers {
        let did = peer.did.clone();
        let confirming = pair.remove_confirm.as_deref() == Some(peer.did.as_str());
        let control: Element<'a, Message> = if confirming {
            row![
                text("确认移除？").size(13).color(colors.error),
                text_button("移除", colors, Message::PairRemoveConfirm(did)),
                text_button("取消", colors, Message::PairRemoveCancel),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
        } else {
            text_button("移除", colors, Message::PairRemoveRequest(did)).into()
        };
        // 状态胶囊：在线 / 附近 / 离线 / 未知。拿不到输入法快照时是「未知」，
        // 不能画成「离线」——「输入法没开」和「对方不在」是两件事。
        let (badge_label, fg, bg) = match peer.state {
            P2pPeerState::Online => ("在线", colors.success, colors.success_dim),
            P2pPeerState::Nearby => ("附近", colors.tertiary, colors.tertiary_dim),
            P2pPeerState::Offline => ("离线", colors.foreground_muted, colors.surface_variant),
            P2pPeerState::Unknown => ("未知", colors.foreground_faint, colors.surface_variant),
        };
        let desc = match (&peer.state, &peer.last_addr) {
            (P2pPeerState::Online, _) => format!("{} · 已连接，剪贴板会立即同步", peer.short),
            (P2pPeerState::Nearby, _) => format!("{} · 在局域网内，但没连上本机", peer.short),
            (P2pPeerState::Offline, _) => match peer.last_seen {
                Some(ts) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(ts);
                    let ago = (now.saturating_sub(ts).max(0)) as u64;
                    format!(
                        "{} · 离线，上次在线 {}",
                        peer.short,
                        crate::state::relative_time_ago(std::time::Duration::from_secs(ago))
                    )
                }
                None => match &peer.last_addr {
                    Some(addr) => format!("{} · 离线 · 最近地址 {addr}", peer.short),
                    None => format!("{} · 离线（还没有连上过）", peer.short),
                },
            },
            (P2pPeerState::Unknown, _) => {
                format!("{} · 输入法没在运行，看不到状态", peer.short)
            }
        };
        let control = row![badge(badge_label, fg, bg), control]
            .spacing(8)
            .align_y(Alignment::Center)
            .into();
        items.push(settings_item(
            peer.name.clone(),
            Some(desc),
            colors,
            control,
        ));
    }

    settings_group(
        "已配对设备",
        Some("剪贴板只在这些设备之间同步；移除后，这台设备就再也连不上了"),
        colors,
        items,
    )
}
