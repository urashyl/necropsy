/* svcupd: a beacon, as recovered from an infected host. Built here from source so the
 * lab has a real ELF to analyse. It does not persist, spread or damage anything: it
 * decrypts its configuration, builds a request for its gateway and tries to resolve
 * the gateway's name, which ends in .invalid and so never resolves.
 *
 * Its configuration is not in the source: it is patched into CFG when the lab starts,
 * encrypted with RC4 under a key the program assembles at run time. */
#include <netdb.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ptrace.h>
#include <sys/socket.h>
#include <unistd.h>

/* marker (8), length (u16 LE), RC4 ciphertext. Patched in place by the server. */
unsigned char CFG[256] = {0x4e, 0x43, 0x46, 0x47, 0xa5, 0x5a, 0xc3, 0x3c, 0, 0};

/* The RC4 key, XORed with 0x5c, and the gateway name, XORed with 0x37. */
static const unsigned char K_ENC[12] = {0x2a, 0x39, 0x32, 0x38, 0x33, 0x2e, 0x2f, 0x25, 0x32, 0x3f, 0x6e, 0x6b};
static const unsigned char H_ENC[22] = {0x50, 0x40, 0x19, 0x54, 0x53, 0x59, 0x1a, 0x5a, 0x52, 0x43, 0x45, 0x5e, 0x54, 0x44, 0x19, 0x5e, 0x59, 0x41, 0x56, 0x5b, 0x5e, 0x53};

static void rc4(const unsigned char *key, size_t klen, unsigned char *buf, size_t n) {
    unsigned char s[256], t;
    unsigned i, j = 0;
    for (i = 0; i < 256; i++) s[i] = (unsigned char)i;
    for (i = 0; i < 256; i++) {
        j = (j + s[i] + key[i % klen]) & 255;
        t = s[i]; s[i] = s[j]; s[j] = t;
    }
    i = j = 0;
    for (size_t k = 0; k < n; k++) {
        i = (i + 1) & 255;
        j = (j + s[i]) & 255;
        t = s[i]; s[i] = s[j]; s[j] = t;
        buf[k] ^= s[(s[i] + s[j]) & 255];
    }
}

static const char *field(const char *cfg, const char *name, char *out, size_t cap) {
    size_t l = strlen(name);
    for (const char *p = cfg; *p;) {
        const char *e = strchr(p, '\n');
        size_t len = e ? (size_t)(e - p) : strlen(p);
        if (len > l && !strncmp(p, name, l) && p[l] == '=') {
            size_t v = len - l - 1;
            if (v >= cap) v = cap - 1;
            memcpy(out, p + l + 1, v);
            out[v] = 0;
            return out;
        }
        if (!e) break;
        p = e + 1;
    }
    out[0] = 0;
    return out;
}

int main(void) {
    /* Anti-debugging: a traced process cannot be traced again. */
    if (ptrace(PTRACE_TRACEME, 0, 0, 0) < 0) return 0;

    unsigned char key[12], cfg[256];
    for (int i = 0; i < 12; i++) key[i] = K_ENC[i] ^ 0x5c;

    unsigned n = CFG[8] | (CFG[9] << 8);
    if (n == 0 || n > 240) return 0;
    memcpy(cfg, CFG + 10, n);
    rc4(key, sizeof key, cfg, n);
    cfg[n] = 0;

    char host[sizeof H_ENC + 1];
    for (size_t i = 0; i < sizeof H_ENC; i++) host[i] = (char)(H_ENC[i] ^ 0x37);
    host[sizeof H_ENC] = 0;

    char path[64], auth[64], ua[96], req[512];
    field((const char *)cfg, "path", path, sizeof path);
    field((const char *)cfg, "auth", auth, sizeof auth);
    field((const char *)cfg, "ua", ua, sizeof ua);
    snprintf(req, sizeof req,
             "GET %s/hello HTTP/1.1\r\nHost: %s\r\nUser-Agent: %s\r\nX-Auth: %s\r\n\r\n",
             path, host, ua, auth);

#ifdef SVCUPD_DEBUG
    fputs(req, stderr); /* for the lab's own test only: not in the shipped build */
#endif
    struct addrinfo hints, *res = 0;
    memset(&hints, 0, sizeof hints);
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo(host, "443", &hints, &res) != 0) {
        sleep(1);
        return 0;
    }
    /* The real beacon would now connect and send req. */
    freeaddrinfo(res);
    return (int)strlen(req) > 0 ? 0 : 1;
}
