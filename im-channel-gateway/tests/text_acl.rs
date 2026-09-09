use im_channel_gateway::acl::AccessControl;
use im_channel_gateway::text::{split_text, split_text_chars};

#[test]
fn split_text_chunks() {
    let s = "a".repeat(100);
    let chunks = split_text(&s, 40);
    assert!(chunks.len() >= 3);
    assert_eq!(chunks.join(""), s);
}

#[test]
fn split_text_chars_respects_limit() {
    let s = format!("{}\n\n{}", "段落一".repeat(400), "段落二".repeat(400));
    let chunks = split_text_chars(&s, 500);
    assert!(chunks.len() >= 2);
    assert!(chunks.iter().all(|c| c.chars().count() <= 500));
    assert_eq!(
        chunks.iter().map(|c| c.chars().count()).sum::<usize>(),
        s.chars().count()
    );
}

#[test]
fn acl_open_when_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let acl = AccessControl::load_or_create(dir.path()).unwrap();
    assert!(!acl.check_blocked_with_flags("telegram", "user1", false, false, false));
}

#[test]
fn acl_dynamic_account_channel_falls_back_to_base_channel() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("access_control.json"),
        r#"{
          "channels": {
            "telegram": { "whitelist": { "user1": "ok" }, "blacklist": {} },
            "wechat": { "whitelist": { "wx_user": "ok" }, "blacklist": {} }
          }
        }"#,
    )
    .unwrap();
    let acl = AccessControl::load_or_create(dir.path()).unwrap();

    assert!(!acl.check_blocked_with_flags("telegram:tg_123", "user1", true, false, false));
    assert!(acl.check_blocked_with_flags("telegram:tg_123", "user2", true, false, false));
    assert!(!acl.check_blocked_with_flags("wechat:acc_123", "wx_user", true, false, false));
}
