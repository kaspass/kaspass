//! The bot against a stranger who adds it to their own group (Fable's review of update 88, M3).
//! A stand-in for Telegram's API runs on 127.0.0.1; nothing outside this machine is contacted.

use super::*;
use std::sync::Arc;

type Calls = Arc<std::sync::Mutex<Vec<(String, Value)>>>;

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A tiny local HTTP server that answers like Telegram's bot API and writes down every call.
/// Telegram user 666 is the creator of every group; everyone else is not in any group.
fn fake_telegram() -> (String, Calls) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let calls: Calls = Default::default();
    let log = calls.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(mut c) = conn else { continue };
            let log = log.clone();
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let (head_end, len) = loop {
                        if let Some(p) = find(&buf, b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                            let len = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                            if buf.len() >= p + 4 + len {
                                break (p, len);
                            }
                        }
                        match c.read(&mut tmp) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let body = buf[head_end + 4..head_end + 4 + len].to_vec();
                    buf.drain(..head_end + 4 + len);
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    let method = path.rsplit('/').next().unwrap_or("").to_string();
                    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    log.lock().unwrap().push((method.clone(), req.clone()));
                    let result = match method.as_str() {
                        "getMe" => json!({ "id": 1, "username": "kv_members_bot" }),
                        "getChatMember" => json!({ "status": if req["user_id"] == 666 { "creator" } else { "left" } }),
                        "createChatInviteLink" => json!({ "invite_link": format!("https://t.me/+fake{}", req["chat_id"]) }),
                        _ => json!(true),
                    };
                    let out = json!({ "ok": true, "result": result }).to_string();
                    let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}", out.len(), out);
                    if c.write_all(resp.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), calls)
}

/// Someone who isn't the site's owner adds the site's bot to their own group, makes it an admin
/// and sends `/kaspass pass`. The group waits for the owner's approval, and no member is invited to
/// it until then.
#[tokio::test]
async fn a_strangers_group_waits_for_the_owners_approval() {
    let (api, calls) = fake_telegram();
    // Test-only: point the bot at the stand-in API (the code ignores this switch on mainnet).
    unsafe { std::env::set_var("KASPASS_TELEGRAM_API", &api) };
    let dir = std::env::temp_dir().join(format!("kaspass-telegram-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("telegram.token"), "123456:TEST-TOKEN").unwrap();
    let bot = Bot::open(&dir).await.unwrap().expect("bot on");
    bot.set_choices(vec![("level:aaaa".into(), "Kaspa Vision".into()), ("level:bbbb".into(), "Kaspa Vision + KasTax".into())]);
    let access: Box<AccessCheck> = Box::new(|_address: &str| Box::pin(async { Some(vec!["pass".to_string()]) }));
    let member = "kaspatest:qzxh2qxafsfxshgl2695c26sfr59xju8xvvl82x65cftg6gn9mrlwpd0qe3zu";

    let chat = -1001234567890i64;
    let title = "Kaspa Vision Members (official)";
    bot.handle(&json!({ "update_id": 1, "my_chat_member": { "chat": { "id": chat, "type": "supergroup", "title": title }, "from": { "id": 666 }, "new_chat_member": { "status": "administrator" } } }), &*access).await;
    bot.handle(&json!({ "update_id": 2, "message": { "chat": { "id": chat, "type": "supergroup" }, "from": { "id": 666 }, "text": "/kaspass pass" } }), &*access).await;
    let g = bot.groups().into_iter().find(|g| g.chat_id == chat).unwrap();
    assert_eq!(g.key, None, "not a members' group yet");
    assert_eq!(g.pending.as_deref(), Some("pass"), "waiting for the owner");
    assert!(!bot.has_group());

    bot.invite(4242, 4242, member, &*access).await;
    assert!(!calls.lock().unwrap().iter().any(|(m, b)| m == "createChatInviteLink" && b["chat_id"] == chat), "no invite to an unapproved group");

    // The owner approves it in the dashboard: now members are invited.
    assert!(bot.approve(chat, Some("something else")).await.is_err(), "not what the owner saw");
    let (t, label) = bot.approve(chat, Some("paid members")).await.unwrap();
    assert_eq!(t, title);
    assert!(!label.is_empty());
    assert!(bot.has_group());
    bot.invite(4243, 4243, member, &*access).await;
    assert!(calls.lock().unwrap().iter().any(|(m, b)| m == "createChatInviteLink" && b["chat_id"] == chat));

    // A single-plan site: the first group is suggested for every member, and still waits.
    let chat2 = -1009999i64;
    bot.set_choices(vec![]);
    let dir2 = std::env::temp_dir().join(format!("kaspass-telegram-test2-{}", std::process::id()));
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("telegram.token"), "123456:TEST-TOKEN").unwrap();
    let bot2 = Bot::open(&dir2).await.unwrap().expect("bot on");
    bot2.handle(&json!({ "update_id": 3, "my_chat_member": { "chat": { "id": chat2, "type": "supergroup", "title": "Any group" }, "from": { "id": 666 }, "new_chat_member": { "status": "administrator" } } }), &*access).await;
    let g2 = bot2.groups().into_iter().find(|g| g.chat_id == chat2).unwrap();
    assert_eq!((g2.key.as_deref(), g2.pending.as_deref()), (None, Some("pass")));
    assert!(!bot2.has_group());
}
