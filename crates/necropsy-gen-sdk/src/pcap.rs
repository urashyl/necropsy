// SPDX-License-Identifier: AGPL-3.0-or-later
//! Classic pcap captures of HTTP and DNS: Ethernet, IPv4, TCP and UDP with real
//! handshakes, sequence numbers, ACKs, FINs and checksums, so Wireshark, tshark
//! and the workspace's pcap viewer read them as they would a real capture. The
//! same as `tools/learn/pcaplib.py`.

const MAC_CLIENT: [u8; 6] = [0x3c, 0x52, 0x82, 0xa1, 0xb2, 0x0c];
const MAC_GATEWAY: [u8; 6] = [0x00, 0x50, 0x56, 0x9c, 0x1e, 0x01];

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for pair in data.chunks(2) {
        let word = u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]);
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// An IPv4 address from `"a.b.c.d"`. Panics on anything else.
fn ip(addr: &str) -> [u8; 4] {
    let mut out = [0u8; 4];
    let parts: Vec<&str> = addr.split('.').collect();
    assert_eq!(parts.len(), 4, "not an IPv4 address: {addr}");
    for (o, p) in out.iter_mut().zip(parts) {
        *o = p
            .parse()
            .unwrap_or_else(|_| panic!("not an IPv4 address: {addr}"));
    }
    out
}

pub struct Capture {
    /// Microseconds since the epoch.
    t: u64,
    packets: Vec<(u64, Vec<u8>)>,
    ip_id: u16,
}

impl Capture {
    /// A capture starting at `start` (seconds since the epoch).
    pub fn new(start: u64) -> Self {
        Self {
            t: start * 1_000_000,
            packets: Vec::new(),
            ip_id: 0x1a2b,
        }
    }

    /// Moves the clock on by `micros`.
    pub fn wait(&mut self, micros: u64) {
        self.t += micros;
    }

    fn frame(&mut self, src: &str, dst: &str, proto: u8, l4: &[u8], outbound: bool) -> Vec<u8> {
        self.ip_id = self.ip_id.wrapping_add(1);
        let mut hdr = Vec::with_capacity(20);
        hdr.extend_from_slice(&[0x45, 0]);
        hdr.extend_from_slice(&((20 + l4.len()) as u16).to_be_bytes());
        hdr.extend_from_slice(&self.ip_id.to_be_bytes());
        hdr.extend_from_slice(&[0x40, 0, 64, proto, 0, 0]);
        hdr.extend_from_slice(&ip(src));
        hdr.extend_from_slice(&ip(dst));
        let c = checksum(&hdr);
        hdr[10..12].copy_from_slice(&c.to_be_bytes());
        let (s, d) = if outbound {
            (MAC_CLIENT, MAC_GATEWAY)
        } else {
            (MAC_GATEWAY, MAC_CLIENT)
        };
        let mut f = Vec::with_capacity(14 + hdr.len() + l4.len());
        f.extend_from_slice(&d);
        f.extend_from_slice(&s);
        f.extend_from_slice(&[0x08, 0x00]);
        f.extend_from_slice(&hdr);
        f.extend_from_slice(l4);
        f
    }

    fn add(&mut self, frame: Vec<u8>, dt: u64) {
        self.t += dt;
        self.packets.push((self.t, frame));
    }

    #[allow(clippy::too_many_arguments)]
    fn tcp(
        &mut self,
        src: &str,
        dst: &str,
        ports: (u16, u16),
        seq: u32,
        ack: u32,
        flags: u8,
        payload: &[u8],
        outbound: bool,
    ) -> Vec<u8> {
        let mut t = Vec::with_capacity(20 + payload.len());
        t.extend_from_slice(&ports.0.to_be_bytes());
        t.extend_from_slice(&ports.1.to_be_bytes());
        t.extend_from_slice(&seq.to_be_bytes());
        t.extend_from_slice(&ack.to_be_bytes());
        t.extend_from_slice(&[5 << 4, flags]);
        t.extend_from_slice(&64240u16.to_be_bytes());
        t.extend_from_slice(&[0, 0, 0, 0]);
        t.extend_from_slice(payload);
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&ip(src));
        pseudo.extend_from_slice(&ip(dst));
        pseudo.extend_from_slice(&[0, 6]);
        pseudo.extend_from_slice(&(t.len() as u16).to_be_bytes());
        pseudo.extend_from_slice(&t);
        let c = checksum(&pseudo);
        t[16..18].copy_from_slice(&c.to_be_bytes());
        self.frame(src, dst, 6, &t, outbound)
    }

    /// One TCP connection on port 80: handshake, the request, the response, the close.
    pub fn http(
        &mut self,
        client: &str,
        server: &str,
        cport: u16,
        request: &[u8],
        response: &[u8],
    ) {
        const SYN: u8 = 0x02;
        const ACK: u8 = 0x10;
        const PSH: u8 = 0x08;
        const FIN: u8 = 0x01;
        const MSS: usize = 1400;
        let rtt = 20_000;
        let out = (cport, 80u16);
        let back = (80u16, cport);
        let mut c_seq = 0x1000u32.wrapping_add(u32::from(cport).wrapping_mul(7));
        let mut s_seq = 0x9000u32.wrapping_add(u32::from(cport).wrapping_mul(13));
        let f = self.tcp(client, server, out, c_seq, 0, SYN, &[], true);
        self.add(f, 0);
        let f = self.tcp(
            server,
            client,
            back,
            s_seq,
            c_seq + 1,
            SYN | ACK,
            &[],
            false,
        );
        self.add(f, rtt);
        c_seq += 1;
        s_seq += 1;
        let f = self.tcp(client, server, out, c_seq, s_seq, ACK, &[], true);
        self.add(f, 200);
        for part in request.chunks(MSS) {
            let f = self.tcp(client, server, out, c_seq, s_seq, PSH | ACK, part, true);
            self.add(f, 300);
            c_seq += part.len() as u32;
        }
        let f = self.tcp(server, client, back, s_seq, c_seq, ACK, &[], false);
        self.add(f, rtt);
        for part in response.chunks(MSS) {
            let f = self.tcp(server, client, back, s_seq, c_seq, PSH | ACK, part, false);
            self.add(f, 4_000);
            s_seq += part.len() as u32;
        }
        let f = self.tcp(client, server, out, c_seq, s_seq, ACK, &[], true);
        self.add(f, 200);
        let f = self.tcp(client, server, out, c_seq, s_seq, FIN | ACK, &[], true);
        self.add(f, 1_000);
        let f = self.tcp(
            server,
            client,
            back,
            s_seq,
            c_seq + 1,
            FIN | ACK,
            &[],
            false,
        );
        self.add(f, rtt);
        let f = self.tcp(client, server, out, c_seq + 1, s_seq + 1, ACK, &[], true);
        self.add(f, 200);
    }

    /// A DNS A query for `name` and its answer.
    pub fn dns(
        &mut self,
        client: &str,
        resolver: &str,
        cport: u16,
        name: &str,
        answer: &str,
        txid: u16,
    ) {
        let mut q = Vec::new();
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.extend_from_slice(&[0, 0, 1, 0, 1]);
        let mut query = Vec::new();
        query.extend_from_slice(&txid.to_be_bytes());
        query.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
        query.extend_from_slice(&q);
        let mut reply = Vec::new();
        reply.extend_from_slice(&txid.to_be_bytes());
        reply.extend_from_slice(&[0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0]);
        reply.extend_from_slice(&q);
        reply.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 1, 0x2c, 0, 4]);
        reply.extend_from_slice(&ip(answer));
        for (src, dst, sp, dp, data, out, dt) in [
            (client, resolver, cport, 53u16, &query, true, 0u64),
            (resolver, client, 53u16, cport, &reply, false, 12_000),
        ] {
            let mut u = Vec::new();
            u.extend_from_slice(&sp.to_be_bytes());
            u.extend_from_slice(&dp.to_be_bytes());
            u.extend_from_slice(&((8 + data.len()) as u16).to_be_bytes());
            u.extend_from_slice(&[0, 0]);
            u.extend_from_slice(data);
            let mut pseudo = Vec::new();
            pseudo.extend_from_slice(&ip(src));
            pseudo.extend_from_slice(&ip(dst));
            pseudo.extend_from_slice(&[0, 17]);
            pseudo.extend_from_slice(&(u.len() as u16).to_be_bytes());
            pseudo.extend_from_slice(&u);
            let c = match checksum(&pseudo) {
                0 => 0xffff,
                c => c,
            };
            u[6..8].copy_from_slice(&c.to_be_bytes());
            let f = self.frame(src, dst, 17, &u, out);
            self.add(f, dt);
        }
    }

    /// The capture as a classic pcap file (microsecond timestamps, Ethernet).
    pub fn finish(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&65535u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        for (t, f) in &self.packets {
            out.extend_from_slice(&((t / 1_000_000) as u32).to_le_bytes());
            out.extend_from_slice(&((t % 1_000_000) as u32).to_le_bytes());
            out.extend_from_slice(&(f.len() as u32).to_le_bytes());
            out.extend_from_slice(&(f.len() as u32).to_le_bytes());
            out.extend_from_slice(f);
        }
        out
    }
}

/// An HTTP/1.1 request with `Host`, the given headers and `Content-Length` for a body.
pub fn request(
    method: &str,
    path: &str,
    host: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Vec<u8> {
    let mut s = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n");
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        s.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    s.push_str("\r\n");
    let mut out = s.into_bytes();
    out.extend_from_slice(body);
    out
}

/// An HTTP/1.1 response with the given headers and `Content-Length`.
pub fn response(status: u16, reason: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut s = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    s.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut out = s.into_bytes();
    out.extend_from_slice(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_has_the_pcap_header_and_every_packet() {
        let mut c = Capture::new(1_700_000_000);
        c.dns("10.0.0.5", "10.0.0.1", 50000, "x.test", "192.0.2.7", 7);
        let req = request("GET", "/", "x.test", &[("User-Agent", "t")], b"");
        let res = response(200, "OK", &[], b"hi");
        c.http("10.0.0.5", "192.0.2.7", 49000, &req, &res);
        let b = c.finish();
        assert_eq!(&b[..4], &0xa1b2_c3d4u32.to_le_bytes());
        // 2 DNS + 3 handshake + 1 request + 1 ack + 1 response + 1 ack + 3 close
        let mut n = 0;
        let mut at = 24;
        while at < b.len() {
            let len = u32::from_le_bytes(b[at + 8..at + 12].try_into().unwrap()) as usize;
            at += 16 + len;
            n += 1;
        }
        assert_eq!(n, 12);
        assert_eq!(at, b.len());
    }

    #[test]
    fn checksums_verify() {
        // An IP header with its checksum in place sums to zero.
        let mut c = Capture::new(0);
        let f = c.frame("10.0.0.1", "10.0.0.2", 17, &[0; 8], true);
        assert_eq!(checksum(&f[14..34]), 0);
    }
}
