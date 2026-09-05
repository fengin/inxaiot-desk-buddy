use std::net::IpAddr;
use std::sync::OnceLock;

use netdev::{Interface, MacAddr};

/// 本次运行统一使用同一份电脑来源字符串，操作记录和租约不受中途换网影响。
pub fn application_instance_id() -> &'static str {
    static INSTANCE_ID: OnceLock<String> = OnceLock::new();
    INSTANCE_ID.get_or_init(|| {
        let computer_name = hostname::get()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        format_instance_id(
            &computer_name,
            &netdev::get_interfaces(),
            is_hardware_interface,
        )
    })
}

fn interface_ip(interface: &Interface) -> Option<IpAddr> {
    interface
        .ipv4
        .iter()
        .map(|network| network.addr())
        .filter(|ip| {
            !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
        })
        .min_by_key(|ip| (ip.is_link_local(), *ip))
        .map(IpAddr::V4)
        .or_else(|| {
            interface
                .ipv6
                .iter()
                .map(|network| network.addr())
                .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
                .min_by_key(|ip| (ip.is_unicast_link_local(), *ip))
                .map(IpAddr::V6)
        })
}

fn interface_mac(interface: &Interface) -> Option<MacAddr> {
    interface.mac_addr.filter(|mac| *mac != MacAddr::zero())
}

#[cfg(windows)]
fn is_hardware_interface(interface: &Interface) -> bool {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2};
    let mut row = MIB_IF_ROW2 {
        InterfaceIndex: interface.index,
        ..Default::default()
    };
    // GetIfEntry2 只读取指定接口；该结构第 0 位是 HardwareInterface，虚拟网卡为 0。
    unsafe { GetIfEntry2(&mut row) == 0 && row.InterfaceAndOperStatusFlags._bitfield & 1 != 0 }
}

#[cfg(not(windows))]
fn is_hardware_interface(interface: &Interface) -> bool {
    interface.is_physical()
}

fn select_interface(
    interfaces: &[Interface],
    is_hardware: impl Fn(&Interface) -> bool,
) -> Option<&Interface> {
    interfaces
        .iter()
        .filter(|interface| !interface.is_loopback())
        .min_by_key(|interface| {
            (
                !interface.is_up(),
                interface_ip(interface).is_none(),
                interface_mac(interface).is_none(),
                !is_hardware(interface),
                !interface.default,
                interface.index,
            )
        })
}

fn format_instance_id(
    computer_name: &str,
    interfaces: &[Interface],
    is_hardware: impl Fn(&Interface) -> bool,
) -> String {
    let computer_name: String = computer_name
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let computer_name = if computer_name.is_empty() {
        "未知电脑"
    } else {
        &computer_name
    };
    // MAC 和 IP 始终来自同一网卡；先选在线物理网卡，再按默认路由排序。
    let interface = select_interface(interfaces, is_hardware);
    let mac = interface
        .and_then(interface_mac)
        .map(|mac| mac.to_string().replace(':', "").to_uppercase())
        .unwrap_or_else(|| "未知MAC".into());
    let ip = interface
        .and_then(interface_ip)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "未联网".into());
    format!("{computer_name}-{mac}-{ip}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdev::interface::flags::{IFF_LOOPBACK, IFF_UP};

    fn adapter(index: u32, mac: [u8; 6], ip: &str, default: bool) -> Interface {
        let mut interface = Interface::dummy();
        interface.index = index;
        interface.flags = IFF_UP;
        interface.mac_addr = Some(MacAddr::from_octets(mac));
        interface.ipv4 = vec![ip.parse().expect("IPv4 network")];
        interface.default = default;
        interface
    }

    #[test]
    fn default_adapter_supplies_matching_mac_and_ip_without_truncating_name() {
        let mut loopback = adapter(1, [0; 6], "127.0.0.1/8", true);
        loopback.flags |= IFF_LOOPBACK;
        let virtual_adapter = adapter(2, [2, 1, 2, 3, 4, 5], "10.10.0.1/24", false);
        let physical = adapter(5, [0, 17, 34, 170, 187, 204], "192.168.3.142/24", true);
        let hostname =
            "LF-PC-完整电脑名称不截断-ABCDEFGHIJKLMNOPQRSTUVWXYZ-ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let interfaces = [loopback, virtual_adapter, physical];
        let expected = format!("{hostname}-001122AABBCC-192.168.3.142");
        assert!(expected.chars().count() > 64);
        assert_eq!(
            format_instance_id(hostname, &interfaces, |_| false),
            expected
        );
    }

    #[test]
    fn missing_mac_on_default_tunnel_uses_complete_adapter_without_mixing_values() {
        let tunnel = adapter(1, [0; 6], "10.0.0.1/24", true);
        let ethernet = adapter(2, [0, 17, 34, 170, 187, 204], "192.168.3.142/24", false);
        assert_eq!(
            format_instance_id("LF-PC", &[tunnel, ethernet], |_| false),
            "LF-PC-001122AABBCC-192.168.3.142"
        );
    }

    #[test]
    fn physical_adapter_wins_over_virtual_default_route() {
        let virtual_adapter = adapter(1, [34, 127, 201, 156, 230, 203], "10.0.0.37/24", true);
        let physical = adapter(2, [0, 17, 34, 170, 187, 204], "192.168.3.142/24", false);
        let interfaces = [virtual_adapter, physical];
        assert_eq!(
            select_interface(&interfaces, |interface| interface.index == 2)
                .map(|interface| interface.index),
            Some(2)
        );
    }

    #[test]
    fn offline_and_ipv6_sources_remain_readable() {
        let mut interface = Interface::dummy();
        interface.mac_addr = Some(MacAddr::from_octets([0, 17, 34, 170, 187, 204]));
        assert_eq!(
            format_instance_id("LF-PC", &[interface.clone()], |_| false),
            "LF-PC-001122AABBCC-未联网"
        );
        interface.ipv6 = vec!["2001:db8::142/64".parse().expect("IPv6 network")];
        assert_eq!(
            format_instance_id("LF-PC", &[interface], |_| false),
            "LF-PC-001122AABBCC-2001:db8::142"
        );
        assert_eq!(
            format_instance_id("", &[], |_| false),
            "未知电脑-未知MAC-未联网"
        );
    }
}
