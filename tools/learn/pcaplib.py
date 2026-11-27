# SPDX-License-Identifier: AGPL-3.0-or-later
"""A small writer for the HTTP captures in the Learn practice packs.

It writes classic pcap (Ethernet, IPv4, TCP and UDP) with real handshakes,
sequence numbers, ACKs, FINs and checksums, so Wireshark, tshark and the
workspace's pcap viewer read the files as they would a real capture. Output is
deterministic: the same script makes the same bytes. See docs/learn.md.
"""

import struct

ETH_CLIENT = bytes.fromhex("3c5282a1b20c")
ETH_GATEWAY = bytes.fromhex("0050569c1e01")


def _ip(addr):
    return bytes(int(x) for x in addr.split("."))


def _csum(data):
    if len(data) % 2:
        data += b"\0"
    s = sum(struct.unpack(f"!{len(data) // 2}H", data))
    while s >> 16:
        s = (s & 0xFFFF) + (s >> 16)
    return (~s) & 0xFFFF


class Capture:
    def __init__(self, start):
        self.t = float(start)
        self.packets = []
        self.ip_id = 0x1A2B

    def _frame(self, src, dst, proto, l4, src_mac, dst_mac):
        self.ip_id = (self.ip_id + 1) & 0xFFFF
        hdr = struct.pack("!BBHHHBBH4s4s", 0x45, 0, 20 + len(l4), self.ip_id, 0x4000, 64, proto, 0, _ip(src), _ip(dst))
        hdr = hdr[:10] + struct.pack("!H", _csum(hdr)) + hdr[12:]
        return dst_mac + src_mac + b"\x08\x00" + hdr + l4

    def _add(self, frame, dt):
        self.t += dt
        self.packets.append((self.t, frame))

    def _tcp(self, src, dst, sport, dport, seq, ack, flags, payload, outbound):
        off = 5 << 4
        tcp = struct.pack("!HHIIBBHHH", sport, dport, seq, ack, off, flags, 64240, 0, 0) + payload
        pseudo = _ip(src) + _ip(dst) + struct.pack("!BBH", 0, 6, len(tcp))
        tcp = tcp[:16] + struct.pack("!H", _csum(pseudo + tcp)) + tcp[18:]
        macs = (ETH_CLIENT, ETH_GATEWAY) if outbound else (ETH_GATEWAY, ETH_CLIENT)
        return self._frame(src, dst, 6, tcp, *macs)

    def wait(self, seconds):
        self.t += seconds

    def http(self, client, server, cport, request, response, sport=80, rtt=0.02, mss=1400):
        """One TCP connection: handshake, the request, the response, and the close."""
        if isinstance(request, str):
            request = request.encode()
        if isinstance(response, str):
            response = response.encode()
        c_seq, s_seq = 0x1000 + cport * 7, 0x9000 + cport * 13
        SYN, ACK, PSH, FIN = 0x02, 0x10, 0x08, 0x01
        self._add(self._tcp(client, server, cport, sport, c_seq, 0, SYN, b"", True), 0)
        self._add(self._tcp(server, client, sport, cport, s_seq, c_seq + 1, SYN | ACK, b"", False), rtt)
        c_seq += 1
        s_seq += 1
        self._add(self._tcp(client, server, cport, sport, c_seq, s_seq, ACK, b"", True), 0.0002)
        for i in range(0, len(request), mss):
            part = request[i:i + mss]
            self._add(self._tcp(client, server, cport, sport, c_seq, s_seq, PSH | ACK, part, True), 0.0003)
            c_seq += len(part)
        self._add(self._tcp(server, client, sport, cport, s_seq, c_seq, ACK, b"", False), rtt)
        for i in range(0, len(response), mss):
            part = response[i:i + mss]
            self._add(self._tcp(server, client, sport, cport, s_seq, c_seq, PSH | ACK, part, False), 0.004)
            s_seq += len(part)
        self._add(self._tcp(client, server, cport, sport, c_seq, s_seq, ACK, b"", True), 0.0002)
        self._add(self._tcp(client, server, cport, sport, c_seq, s_seq, FIN | ACK, b"", True), 0.001)
        self._add(self._tcp(server, client, sport, cport, s_seq, c_seq + 1, FIN | ACK, b"", False), rtt)
        self._add(self._tcp(client, server, cport, sport, c_seq + 1, s_seq + 1, ACK, b"", True), 0.0002)

    def dns(self, client, resolver, cport, name, answer, txid):
        """A DNS A query and its answer."""
        q = b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\0" + struct.pack("!HH", 1, 1)
        query = struct.pack("!HHHHHH", txid, 0x0100, 1, 0, 0, 0) + q
        reply = struct.pack("!HHHHHH", txid, 0x8180, 1, 1, 0, 0) + q
        reply += struct.pack("!HHHIH", 0xC00C, 1, 1, 300, 4) + _ip(answer)
        for src, dst, sp, dp, data, out, dt in (
            (client, resolver, cport, 53, query, True, 0),
            (resolver, client, 53, cport, reply, False, 0.012),
        ):
            udp = struct.pack("!HHHH", sp, dp, 8 + len(data), 0) + data
            pseudo = _ip(src) + _ip(dst) + struct.pack("!BBH", 0, 17, len(udp))
            udp = udp[:6] + struct.pack("!H", _csum(pseudo + udp) or 0xFFFF) + udp[8:]
            macs = (ETH_CLIENT, ETH_GATEWAY) if out else (ETH_GATEWAY, ETH_CLIENT)
            self._add(self._frame(src, dst, 17, udp, *macs), dt)

    def write(self, path):
        out = struct.pack("<IHHiIII", 0xA1B2C3D4, 2, 4, 0, 0, 65535, 1)
        for t, frame in self.packets:
            sec = int(t)
            usec = int(round((t - sec) * 1_000_000))
            if usec >= 1_000_000:
                sec, usec = sec + 1, usec - 1_000_000
            out += struct.pack("<IIII", sec, usec, len(frame), len(frame)) + frame
        with open(path, "wb") as f:
            f.write(out)


def request(method, path, host, headers=(), body=b""):
    if isinstance(body, str):
        body = body.encode()
    lines = [f"{method} {path} HTTP/1.1", f"Host: {host}"]
    lines += [f"{k}: {v}" for k, v in headers]
    if body:
        lines.append(f"Content-Length: {len(body)}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode() + body


def response(status, reason, headers=(), body=b""):
    if isinstance(body, str):
        body = body.encode()
    lines = [f"HTTP/1.1 {status} {reason}"]
    lines += [f"{k}: {v}" for k, v in headers]
    lines.append(f"Content-Length: {len(body)}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode() + body
