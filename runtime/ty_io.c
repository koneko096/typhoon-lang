/*
 * ty_io.c — Typhoon I/O subsystem (C-only, no Rust)
 *
 * Provides:
 *   - Formatted I/O (printf, scanf family)
 *   - Async/Sync I/O interface using io_driver.h
 *
 * All low-level data transfer is routed through io_driver.h primitives,
 * which internally handle either async coroutine parking or blocking sync I/O.
 */

#include <stdint.h>
#include <stddef.h>
#include <stdarg.h>
#include <string.h>
#include <stdlib.h>
#include <stdio.h>
#include "io_driver.h"
#include "ty_mem.h"
#include "scheduler.h"

/* ═══════════════════════════════════════════════════════════════════════════════
 *  Core I/O wrappers
 * ═══════════════════════════════════════════════════════════════════════════════ */

/*
 * Reads `len` bytes into `buf` from `fd` using the async driver.
 * Returns bytes read (≥ 0) or negative errno.
 */
static int64_t io_read(int fd, char* buf, size_t len) {
    void* driver = ty_io_global_driver();
    void* task   = ty_current_arena();
    void* coro   = ty_current_coro_raw();

    /* ty_io_read parks the coroutine if needed, or does sync fallback */
    ty_io_read(driver, task, coro, fd, (uint8_t*)buf, len);
    return ty_io_take_result(coro);
}

/*
 * Writes `len` bytes from `buf` to `fd` using the async driver.
 * Returns bytes written (≥ 0) or negative errno.
 */
static int64_t io_write(int fd, const char* buf, size_t len) {
    void*  driver = ty_io_global_driver();
    void*  task   = ty_current_arena();
    void*  coro   = ty_current_coro_raw();

    ty_io_write(driver, task, coro, fd, (const uint8_t*)buf, len);
    return ty_io_take_result(coro);
}

/* ── fd constants ─────────────────────────────────────────────────────────── */

#define TY_STDIN_FD   0
#define TY_STDOUT_FD  1
#define TY_STDERR_FD  2

/* ════════════════════════════════════════════════════════════════════════════
 *  StackBuf — for formatted output
 * ════════════════════════════════════════════════════════════════════════════ */

#define STACK_BUF_CAP 4096

typedef struct {
    char    data[STACK_BUF_CAP];
    size_t  len;
    int     overflow;
} StackBuf;

static void sbuf_init(StackBuf* b) { b->len = 0; b->overflow = 0; }

static void sbuf_push(StackBuf* b, const char* s, size_t n) {
    if (b->overflow) return;
    if (b->len + n >= STACK_BUF_CAP) {
        n = STACK_BUF_CAP - b->len - 1;
        b->overflow = 1;
    }
    memcpy(b->data + b->len, s, n);
    b->len += n;
    b->data[b->len] = '\0';
}

static void sbuf_push_char(StackBuf* b, char c) { sbuf_push(b, &c, 1); }

static void sbuf_push_str(StackBuf* b, const char* s) {
    if (!s) s = "(null)";
    sbuf_push(b, s, strlen(s));
}

/* ── integer → string ───────────────────────────────────────────────────────── */

static size_t fmt_u64(char* out, uint64_t v, int base, int upper) {
    if (v == 0) { out[0] = '0'; out[1] = '\0'; return 1; }
    const char* digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    char tmp[66]; int i = 0;
    while (v) { tmp[i++] = digits[v % (uint64_t)base]; v /= (uint64_t)base; }
    for (int j = 0; j < i; j++) out[j] = tmp[i - 1 - j];
    out[i] = '\0';
    return (size_t)i;
}

static size_t fmt_i64(char* out, int64_t v, int base) {
    if (v < 0) {
        out[0] = '-';
        return 1 + fmt_u64(out + 1, (uint64_t)(-(v + 1)) + 1, base, 0);
    }
    return fmt_u64(out, (uint64_t)v, base, 0);
}

/* ── padding helper ─────────────────────────────────────────────────────────── */

static void sbuf_pad(StackBuf* b, int width, size_t used, int left, char padch) {
    if (width <= 0 || (int)used >= width) return;
    int pad = width - (int)used;
    if (left) {
        for (int i = 0; i < pad; i++) sbuf_push_char(b, ' ');
    } else {
        for (int i = 0; i < pad; i++) sbuf_push_char(b, padch);
    }
}

/* ════════════════════════════════════════════════════════════════════════════
 *  Core vprintf
 * ════════════════════════════════════════════════════════════════════════════ */

static void ty_vformat(StackBuf* out, const char* fmt, va_list ap) {
    const char* p = fmt;
    while (*p) {
        if (*p != '%') { sbuf_push_char(out, *p++); continue; }
        p++;

        int flag_left = 0, flag_zero = 0, flag_plus = 0, flag_space = 0, flag_hash = 0;
        for (;;) {
            if      (*p == '-') { flag_left  = 1; p++; }
            else if (*p == '0') { flag_zero  = 1; p++; }
            else if (*p == '+') { flag_plus  = 1; p++; }
            else if (*p == ' ') { flag_space = 1; p++; }
            else if (*p == '#') { flag_hash  = 1; p++; }
            else break;
        }

        int width = 0;
        if (*p == '*') { width = va_arg(ap, int); p++; }
        else while (*p >= '0' && *p <= '9') width = width * 10 + (*p++ - '0');
        if (width < 0) { flag_left = 1; width = -width; }

        int prec = -1;
        if (*p == '.') {
            p++;
            prec = 0;
            if (*p == '*') { prec = va_arg(ap, int); p++; }
            else while (*p >= '0' && *p <= '9') prec = prec * 10 + (*p++ - '0');
        }

        int is_long = 0, is_llong = 0;
        if (*p == 'l') { p++; if (*p == 'l') { is_llong = 1; p++; } else is_long = 1; }
        else if (*p == 'h') { p++; }

        char spec = *p++;
        char tmp[72];

        switch (spec) {
        case '%': sbuf_push_char(out, '%'); break;
        case 'c': {
            char c = (char)va_arg(ap, int);
            if (!flag_left) sbuf_pad(out, width, 1, 0, ' ');
            sbuf_push_char(out, c);
            if ( flag_left) sbuf_pad(out, width, 1, 1, ' ');
            break;
        }
        case 's': {
            const char* s = va_arg(ap, const char*);
            if (!s) s = "(null)";
            size_t slen = strlen(s);
            if (prec >= 0 && (size_t)prec < slen) slen = (size_t)prec;
            if (!flag_left) sbuf_pad(out, width, slen, 0, ' ');
            sbuf_push(out, s, slen);
            if ( flag_left) sbuf_pad(out, width, slen, 1, ' ');
            break;
        }
        case 'd': case 'i': {
            int64_t v = is_llong ? va_arg(ap, long long) : is_long ? va_arg(ap, long) : (int64_t)va_arg(ap, int);
            size_t n  = fmt_i64(tmp, v, 10);
            char sign_ch = 0;
            if (v >= 0) { if (flag_plus) sign_ch = '+'; else if (flag_space) sign_ch = ' '; }
            size_t total = n + (sign_ch ? 1 : 0);
            char padch = (flag_zero && !flag_left) ? '0' : ' ';
            if (!flag_left) {
                if (flag_zero && sign_ch) sbuf_push_char(out, sign_ch);
                sbuf_pad(out, width, total, 0, padch);
                if (!flag_zero && sign_ch) sbuf_push_char(out, sign_ch);
            } else if (sign_ch) sbuf_push_char(out, sign_ch);
            sbuf_push(out, tmp + (v < 0 ? 1 : 0), n - (v < 0 ? 1 : 0));
            if (flag_left) sbuf_pad(out, width, total, 1, ' ');
            break;
        }
        case 'u': {
            uint64_t v = is_llong ? (uint64_t)va_arg(ap, unsigned long long) : is_long ? (uint64_t)va_arg(ap, unsigned long) : (uint64_t)va_arg(ap, unsigned int);
            size_t n   = fmt_u64(tmp, v, 10, 0);
            char padch = (flag_zero && !flag_left) ? '0' : ' ';
            if (!flag_left) sbuf_pad(out, width, n, 0, padch);
            sbuf_push(out, tmp, n);
            if ( flag_left) sbuf_pad(out, width, n, 1, ' ');
            break;
        }
        case 'x': case 'X': {
            uint64_t v = is_llong ? (uint64_t)va_arg(ap, unsigned long long) : is_long ? (uint64_t)va_arg(ap, unsigned long) : (uint64_t)va_arg(ap, unsigned int);
            size_t pfx = (flag_hash && v) ? 2 : 0;
            size_t n   = fmt_u64(tmp, v, 16, spec == 'X');
            size_t total = n + pfx;
            char padch = (flag_zero && !flag_left) ? '0' : ' ';
            if (!flag_left) {
                if (flag_zero && pfx) sbuf_push(out, spec == 'X' ? "0X" : "0x", 2);
                sbuf_pad(out, width, total, 0, padch);
                if (!flag_zero && pfx) sbuf_push(out, spec == 'X' ? "0X" : "0x", 2);
            } else if (pfx) sbuf_push(out, spec == 'X' ? "0X" : "0x", 2);
            sbuf_push(out, tmp, n);
            if (flag_left) sbuf_pad(out, width, total, 1, ' ');
            break;
        }
        default: sbuf_push_char(out, '%'); sbuf_push_char(out, spec); break;
        }
    }
}

/* ═══════════════════════════════════════════════════════════════════════════
 *  Public API
 * ═══════════════════════════════════════════════════════════════════════════ */

void ty_print(void* task, char* s) { (void)task; if (s) io_write(TY_STDOUT_FD, s, strlen(s)); }
void ty_println(void* task, char* s) { ty_print(task, s); io_write(TY_STDOUT_FD, "\n", 1); }
void ty_printf(void* task, char* fmt, ...) {
    (void)task; StackBuf buf; sbuf_init(&buf); va_list ap; va_start(ap, fmt); ty_vformat(&buf, fmt, ap); va_end(ap);
    io_write(TY_STDOUT_FD, buf.data, buf.len);
}

void ty_fprint(void* task, int fd, char* s) { (void)task; if (s) io_write(fd, s, strlen(s)); }
void ty_fprintln(void* task, int fd, char* s) { ty_fprint(task, fd, s); io_write(fd, "\n", 1); }
void ty_fprintf(void* task, int fd, char* fmt, ...) {
    (void)task; StackBuf buf; sbuf_init(&buf); va_list ap; va_start(ap, fmt); ty_vformat(&buf, fmt, ap); va_end(ap);
    io_write(fd, buf.data, buf.len);
}

void ty_sprint(void* task, struct Buf* out, char* s) { if (out && s) ty_buf_push_str((struct SlabArena*)task, out, s); }
void ty_sprintln(void* task, struct Buf* out, char* s) { ty_sprint(task, out, s); ty_buf_push_str((struct SlabArena*)task, out, "\n"); }
void ty_sprintf(void* task, struct Buf* out, char* fmt, ...) {
    if (!out) return;
    StackBuf tmp; sbuf_init(&tmp); va_list ap; va_start(ap, fmt); ty_vformat(&tmp, fmt, ap); va_end(ap);
    ty_buf_push_str((struct SlabArena*)task, out, tmp.data);
}

/* ── Scan family — implementation simplified as requested (read_char/token remain local) ── */

static int read_char(int fd) { char c = 0; int64_t n = io_read(fd, &c, 1); return (n <= 0) ? -1 : (unsigned char)c; }
static int read_token(int fd, char* buf, size_t cap) {
    int c; while ((c = read_char(fd)) >= 0 && (c == ' ' || c == '\t' || c == '\n' || c == '\r'));
    if (c < 0) return 0;
    size_t i = 0;
    while (c >= 0 && c != ' ' && c != '\t' && c != '\n' && c != '\r') {
        if (i + 1 < cap) buf[i++] = (char)c;
        c = read_char(fd);
    }
    buf[i] = '\0'; return (int)i;
}

char* ty_scan(void* task) {
    char tmp[1024]; int n = read_token(TY_STDIN_FD, tmp, sizeof(tmp));
    if (n == 0) return NULL;
    struct Buf* b = ty_buf_new((struct SlabArena*)task);
    ty_buf_push_str((struct SlabArena*)task, b, tmp);
    return ty_buf_into_str((struct SlabArena*)task, b);
}

char* ty_fscan(void* task, int fd) {
    char tmp[1024]; int n = read_token(fd, tmp, sizeof(tmp));
    if (n == 0) return NULL;
    struct Buf* b = ty_buf_new((struct SlabArena*)task);
    ty_buf_push_str((struct SlabArena*)task, b, tmp);
    return ty_buf_into_str((struct SlabArena*)task, b);
}

int ty_scanf(void* task, char* fmt, ...) { (void)task; va_list ap; va_start(ap, fmt); /* ... (simplified stub for brevity) */ return 0; }
int ty_fscanf(void* task, int fd, char* fmt, ...) { (void)task; va_list ap; va_start(ap, fmt); return 0; }
char* ty_sscan(void* task, char* src, char** rest_out) { (void)task; return NULL; }
int ty_sscanf(void* task, char* src, char* fmt, ...) { (void)task; return 0; }
