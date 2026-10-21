#[path = "common/screen_test_support.rs"]
mod support;

#[test]
fn screen_description_accepts_only_explicit_old_and_new_target_formats() {
    let rows = support::parse_live_screens("\u{feff}10寸屏：02:aa:bb:cc:dd:10 192.0.2.106\n4寸屏：02-AA-BB-CC-DD-74\t192.0.2.74:5555\n4寸屏2：192.0.2.82:5555\n平台主机：不是屏，不解析").unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0].name.as_str(), rows[0].size.as_str(), rows[0].ip.as_str(), rows[0].mac.as_str()), ("10寸屏", "10", "192.0.2.106", "02AABBCCDD10"));
    assert_eq!(rows[1].mac, "02AABBCCDD74");
    assert_eq!(rows[2].ip, "192.0.2.82");
    assert!(rows[2].mac.is_empty());
    assert_eq!(support::parse_live_screens("4寸屏：02aabbccdd82 192.0.2.82").unwrap()[0].mac, "02AABBCCDD82");
}

#[test]
fn screen_description_rejects_duplicate_ip_and_normalized_mac() {
    for value in [
        "4寸屏：192.0.2.1:5555\n10寸屏：02:AA:BB:CC:DD:10 192.0.2.1",
        "4寸屏：02:AA:BB:CC:DD:10 192.0.2.1\n10寸屏：02-aa-bb-cc-dd-10 192.0.2.2:5555",
        "4寸屏：02AABBCCDD10 192.0.2.1\n4寸屏2：02:aa:bb:cc:dd:10 192.0.2.2",
    ] {
        assert!(support::parse_live_screens(value).is_err());
    }
}

#[test]
fn screen_description_rejects_invalid_mac_without_guessing_an_identity() {
    for mac in [
        "00:00:00:00:00:00", "FF:FF:FF:FF:FF:FF", "01:AA:BB:CC:DD:EE",
        "02:AA:BB:CC:DD", "02:AA:BB:CC:DD:GG", "02-AA:BB:CC:DD:EE",
        "02:A:BB:CC:DD:EE", "02AABBCCDDEE0", "02AA.BBCC.DDEE",
    ] {
        assert!(support::parse_live_screens(&format!("4寸屏：{mac} 192.0.2.1")).is_err());
    }
}

#[test]
fn screen_description_rejects_ports_hosts_extra_tokens_and_incomplete_lines() {
    for target in [
        "192.0.2.1", "192.0.2.1:5556", "192.0.2.1:0", "[::1]:5555",
        "02:AA:BB:CC:DD:10 192.0.2.1:5556", "02:AA:BB:CC:DD:10 192.0.2.999",
        "02:AA:BB:CC:DD:10 screen.local", "02:AA:BB:CC:DD:10 http://192.0.2.1",
        "02:AA:BB:CC:DD:10 192.0.2.1:5555 extra", "",
    ] {
        assert!(support::parse_live_screens(&format!("4寸屏：{target}")).is_err());
    }
    assert!(support::parse_live_screens("4寸屏 192.0.2.1:5555").is_err());
    assert!(support::parse_live_screens("平台主机：192.0.2.1").is_err());
}
