// SPDX-License-Identifier: AGPL-3.0-or-later
//! A minimal IP address or CIDR range, for `NECROPSY_TRUSTED_PROXIES`.

use std::net::IpAddr;
use std::str::FromStr;

/// An address with a prefix length. A bare address means one host (/32 or /128).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    /// Whether `ip` is inside this range. IPv4-mapped IPv6 addresses
    /// (`::ffff:1.2.3.4`, which dual-stack sockets report) count as IPv4.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                masked(u32::from(net).into(), u32::from(ip).into(), self.prefix, 32)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                masked(u128::from(net), u128::from(ip), self.prefix, 128)
            }
            _ => false,
        }
    }
}

/// Whether the first `prefix` of `bits` bits of `a` and `b` agree.
fn masked(a: u128, b: u128, prefix: u8, bits: u32) -> bool {
    let prefix = u32::from(prefix);
    if prefix == 0 {
        return true;
    }
    let shift = bits - prefix;
    (a >> shift) == (b >> shift)
}

impl FromStr for IpNet {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || format!("not an IP address or CIDR range: {s}");
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let parsed: IpAddr = addr.trim().parse().map_err(|_| bad())?;
        let canonical = parsed.to_canonical();
        // A mapped address (::ffff:a.b.c.d/N) is an IPv4 range whose prefix
        // counts the 96 mapping bits too.
        let mapped = parsed.is_ipv6() && canonical.is_ipv4();
        let max = if canonical.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => {
                let p = p.trim().parse::<u8>().map_err(|_| bad())?;
                if mapped {
                    p.checked_sub(96).ok_or_else(bad)?
                } else {
                    p
                }
            }
            None => max,
        };
        if prefix > max {
            return Err(bad());
        }
        Ok(IpNet {
            addr: canonical,
            prefix,
        })
    }
}

/// Parses a comma-separated list, ignoring empty entries.
pub fn parse_list(s: &str) -> Result<Vec<IpNet>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::parse)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }
    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn single_addresses() {
        assert!(net("10.0.0.5").contains(ip("10.0.0.5")));
        assert!(!net("10.0.0.5").contains(ip("10.0.0.6")));
        assert!(net("::1").contains(ip("::1")));
        assert!(!net("::1").contains(ip("::2")));
    }

    #[test]
    fn ranges() {
        assert!(net("10.0.0.0/8").contains(ip("10.255.1.2")));
        assert!(!net("10.0.0.0/8").contains(ip("11.0.0.1")));
        assert!(net("192.168.1.0/24").contains(ip("192.168.1.200")));
        assert!(!net("192.168.1.0/24").contains(ip("192.168.2.1")));
        assert!(net("fe80::/10").contains(ip("fe80::1")));
        assert!(!net("fe80::/10").contains(ip("fec0::1")));
        assert!(net("0.0.0.0/0").contains(ip("203.0.113.9")));
    }

    #[test]
    fn families_do_not_mix_but_mapped_v4_counts_as_v4() {
        assert!(!net("10.0.0.0/8").contains(ip("::1")));
        assert!(!net("::1").contains(ip("127.0.0.1")));
        assert!(net("127.0.0.1").contains(ip("::ffff:127.0.0.1")));
        assert!(net("::ffff:127.0.0.1").contains(ip("127.0.0.1")));
        assert!(net("::ffff:10.0.0.0/104").contains(ip("10.9.9.9")));
        assert!("::ffff:10.0.0.0/64".parse::<IpNet>().is_err());
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "x",
            "1.2.3",
            "1.2.3.4/33",
            "::1/129",
            "1.2.3.4/",
            "1.2.3.4/x",
            "1.2.3.4/-1",
        ] {
            assert!(bad.parse::<IpNet>().is_err(), "{bad}");
        }
    }

    #[test]
    fn lists() {
        let l = parse_list(" 127.0.0.1 , 10.0.0.0/8,, ::1 ").unwrap();
        assert_eq!(l.len(), 3);
        assert!(parse_list("").unwrap().is_empty());
        assert!(parse_list("127.0.0.1, nope").is_err());
    }
}
