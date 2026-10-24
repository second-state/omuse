//! Keyboard input through `Input.dispatchKeyEvent` / `Input.insertText`, with Playwright's key names
//! (`page.keyboard.press("Control+a")`, `keyboard.type(text, delay=…)`).

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, InsertTextParams,
};
use chromiumoxide::keys::get_key_definition;

use crate::pages::{CdpResult, TaskPage};

const MOD_ALT: i64 = 1;
const MOD_CTRL: i64 = 2;
const MOD_META: i64 = 4;
const MOD_SHIFT: i64 = 8;

/// `"Control+Shift+a"` → (modifier bitmask, "a").
fn parse_combo(combo: &str) -> (i64, String) {
    let parts: Vec<&str> = combo.split('+').collect();
    let (mods, key) = match parts.split_last() {
        Some((k, m)) => (m, *k),
        None => (&[][..], combo),
    };
    let mut mask = 0;
    for m in mods {
        mask |= match *m {
            "Control" | "Ctrl" => MOD_CTRL,
            "Alt" => MOD_ALT,
            "Shift" => MOD_SHIFT,
            "Meta" | "Command" | "Cmd" => MOD_META,
            "ControlOrMeta" => {
                if cfg!(target_os = "macos") {
                    MOD_META
                } else {
                    MOD_CTRL
                }
            }
            _ => 0,
        };
    }
    let key = if key.is_empty() && combo.ends_with('+') {
        "+".to_string()
    } else {
        key.to_string()
    };
    (mask, key)
}

struct KeyInfo {
    key: String,
    code: String,
    key_code: i64,
    text: Option<String>,
}

fn key_info(key: &str) -> Option<KeyInfo> {
    if let Some(d) = get_key_definition(key) {
        return Some(KeyInfo {
            key: d.key.to_string(),
            code: d.code.to_string(),
            key_code: d.key_code,
            text: d.text.map(|t| t.to_string()).or_else(|| {
                if d.key.chars().count() == 1 {
                    Some(d.key.to_string())
                } else {
                    None
                }
            }),
        });
    }
    // a bare character that is not in the US layout (CJK, symbols): sent as text only
    if key.chars().count() == 1 {
        return Some(KeyInfo {
            key: key.to_string(),
            code: String::new(),
            key_code: 0,
            text: Some(key.to_string()),
        });
    }
    None
}

/// Press one key or chord like Playwright's `keyboard.press`.
pub async fn press(page: &TaskPage, combo: &str) -> CdpResult<()> {
    let (mods, key) = parse_combo(combo);
    let info = key_info(&key).ok_or_else(|| {
        chromiumoxide::error::CdpError::msg(format!("Unknown key: \"{}\"", combo))
    })?;
    // with Control / Alt / Meta held, no text is typed (a shortcut, not a character)
    let text = if mods & (MOD_CTRL | MOD_ALT | MOD_META) != 0 {
        None
    } else {
        info.text.clone()
    };
    let mut down = DispatchKeyEventParams::new(if text.is_some() {
        DispatchKeyEventType::KeyDown
    } else {
        DispatchKeyEventType::RawKeyDown
    });
    down.modifiers = Some(mods);
    down.key = Some(info.key.clone());
    down.code = Some(info.code.clone());
    down.windows_virtual_key_code = Some(info.key_code);
    down.native_virtual_key_code = Some(info.key_code);
    down.text = text.clone();
    down.unmodified_text = text;
    // macOS Chrome does not treat Ctrl/Cmd+A as select-all unless told the editing command (Playwright does this too)
    if cfg!(target_os = "macos")
        && mods & (MOD_CTRL | MOD_META) != 0
        && info.key.eq_ignore_ascii_case("a")
    {
        down.commands = Some(vec!["selectAll".to_string()]);
    }
    let mut up = DispatchKeyEventParams::new(DispatchKeyEventType::KeyUp);
    up.modifiers = Some(mods);
    up.key = Some(info.key);
    up.code = Some(info.code);
    up.windows_virtual_key_code = Some(info.key_code);
    up.native_virtual_key_code = Some(info.key_code);
    page.page.execute(down).await?;
    page.page.execute(up).await?;
    Ok(())
}

/// Type text character by character like Playwright's `keyboard.type(text, delay=ms)`: characters of the US layout
/// go in as key events (sites that validate on `keydown` see them), anything else through `Input.insertText`.
pub async fn type_text(page: &TaskPage, text: &str, delay_ms: u64) -> CdpResult<()> {
    for ch in text.chars() {
        let s = ch.to_string();
        let key = if ch == '\n' { "Enter" } else { s.as_str() };
        if get_key_definition(key).is_some() {
            press(page, key).await?;
        } else {
            page.page.execute(InsertTextParams::new(s.clone())).await?;
        }
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }
    Ok(())
}

/// `Input.insertText` — Playwright's `fill()` path for the text itself.
pub async fn insert_text(page: &TaskPage, text: &str) -> CdpResult<()> {
    page.page
        .execute(InsertTextParams::new(text.to_string()))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_parse() {
        assert_eq!(parse_combo("Control+a"), (MOD_CTRL, "a".to_string()));
        assert_eq!(parse_combo("Shift+Tab"), (MOD_SHIFT, "Tab".to_string()));
        assert_eq!(parse_combo("Enter"), (0, "Enter".to_string()));
        assert_eq!(
            parse_combo("Control+Shift+ArrowDown"),
            (MOD_CTRL | MOD_SHIFT, "ArrowDown".to_string())
        );
    }

    #[test]
    fn key_table_lookup() {
        let e = key_info("Enter").unwrap();
        assert_eq!(e.key_code, 13);
        assert_eq!(e.text.as_deref(), Some("\r"));
        assert!(key_info("Backspace").unwrap().text.is_none());
        assert_eq!(key_info("a").unwrap().code, "KeyA");
        assert_eq!(key_info("中").unwrap().text.as_deref(), Some("中"));
        assert!(key_info("NoSuchKey").is_none());
    }
}
