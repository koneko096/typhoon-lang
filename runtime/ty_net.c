/*
    * ty_net.c — minimal capability-gated networking for Typhoon
    *
    * Notes:
    * - Uses OS sockets directly (blocking for now).
    * - `task` is accepted for future slab allocation; currently unused.
    * - Address parsing supports \"host:port\" (IPv4 / hostname). IPv6 literals
    *   are not supported yet.
    */

#include "ty_net.h"
#include "scheduler.h"
#include <string.h>
#include <stdlib.h>

#if defined(_WIN32)
#  define WIN32_LEAN_AND_MEAN
#  include <winsock2.h>
#  include <ws2tcpip.h>
#  pragma comment(lib, "Ws2_32.lib")
typedef SOCKET ty_sock_t;
static int32_t ty_net_last_error(void) { return (int32_t)WSAGetLastError(); }
static void ty_sock_close(ty_sock_t s) { closesocket(s); }
#else
#  include <errno.h>
#  include <unistd.h>
#  include <sys/types.h>
#  include <sys/socket.h>
#  include <netdb.h>
#  include <arpa/inet.h>
typedef int ty_sock_t;
static int32_t ty_net_last_error(void) { return (int32_t)errno; }
static void ty_sock_close(ty_sock_t s) { close(s); }
#endif

typedef struct TyResult_i32_i32 {
    uint8_t ok;
    int32_t value;
    int32_t err;
} TyResult_i32_i32;

struct TyNetwork { uint32_t _tag; };
struct TyListener { ty_sock_t sock; };
struct TySocket { ty_sock_t sock; };

static TyNetwork g_net = { 0x4E45544Eu }; /* 'NETN' */

void ty_net_init(void) {
    #if defined(_WIN32)
    WSADATA wsa;
    (void)WSAStartup(MAKEWORD(2, 2), &wsa);
    #endif
}

void ty_net_shutdown(void) {
    #if defined(_WIN32)
    (void)WSACleanup();
    #endif
}

TyNetwork* ty_net_global(void) {
    return &g_net;
}

static int split_host_port(const char* addr, char** host_out, char** port_out) {
    if (!addr) return 0;
    const char* last_colon = strrchr(addr, ':');
    if (!last_colon) return 0;
    size_t host_len = (size_t)(last_colon - addr);
    const char* port = last_colon + 1;
    if (*port == '\0') return 0;

    char* host = (char*)malloc(host_len + 1);
    if (!host) return 0;
    memcpy(host, addr, host_len);
    host[host_len] = '\0';

    *host_out = host;
    *port_out = (char*)port;
    return 1;
}

TyResult_Listener_i32 __ty_method__Network__listen(void* task, TyNetwork* self, char* addr) {
    (void)task;
    (void)self;

    TyResult_Listener_i32 out;
    out.ok = 0;
    out.value = NULL;
    out.err = -1;

    char* host = NULL;
    char* port = NULL;
    if (!split_host_port(addr, &host, &port)) {
        out.err = -2;
        return out;
    }

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    hints.ai_flags = AI_PASSIVE;

    struct addrinfo* res = NULL;
    int gai = getaddrinfo((host[0] == '\0') ? NULL : host, port, &hints, &res);
    if (gai != 0 || !res) {
        free(host);
        out.err = ty_net_last_error();
        return out;
    }

    ty_sock_t s = (ty_sock_t)(-1);
    struct addrinfo* it = res;
    for (; it; it = it->ai_next) {
        s = (ty_sock_t)socket(it->ai_family, it->ai_socktype, it->ai_protocol);
        #if defined(_WIN32)
        if (s == INVALID_SOCKET) continue;
        #else
        if (s < 0) continue;
        #endif

        int yes = 1;
        (void)setsockopt(s, SOL_SOCKET, SO_REUSEADDR, (const char*)&yes, (socklen_t)sizeof(yes));

        if (bind(s, it->ai_addr, (socklen_t)it->ai_addrlen) != 0) {
            ty_sock_close(s);
            continue;
        }
        if (listen(s, 128) != 0) {
            ty_sock_close(s);
            continue;
        }
        break;
    }

    freeaddrinfo(res);
    free(host);

    #if defined(_WIN32)
    if (s == INVALID_SOCKET) {
        out.err = ty_net_last_error();
        return out;
    }
    #else
    if (s < 0) {
        out.err = ty_net_last_error();
        return out;
    }
    #endif

    TyListener* listener = (TyListener*)malloc(sizeof(TyListener));
    if (!listener) {
        ty_sock_close(s);
        out.err = -3;
        return out;
    }
    listener->sock = s;

    out.ok = 1;
    out.value = listener;
    out.err = 0;
    return out;
}

TyResult_Socket_i32 __ty_method__Listener__accept(void* task, TyListener* self) {
    (void)task;
    TyResult_Socket_i32 out;
    out.ok = 0;
    out.value = NULL;
    out.err = -1;

    if (!self) {
        out.err = -2;
        return out;
    }

    ty_sock_t c = (ty_sock_t)(-1);
    c = (ty_sock_t)accept(self->sock, NULL, NULL);
    #if defined(_WIN32)
    if (c == INVALID_SOCKET) {
        out.err = ty_net_last_error();
        return out;
    }
    #else
    if (c < 0) {
        out.err = ty_net_last_error();
        return out;
    }
    #endif

    TySocket* sock = (TySocket*)malloc(sizeof(TySocket));
    if (!sock) {
        ty_sock_close(c);
        out.err = -3;
        return out;
    }
    sock->sock = c;

    out.ok = 1;
    out.value = sock;
    out.err = 0;
    return out;
}

TyResult_i32_i32 __ty_method__Socket__read(void* task, TySocket* self, char* buf, int32_t len) {
    (void)task;
    TyResult_i32_i32 out;
    out.ok = 0;
    out.value = 0;
    out.err = -1;

    if (!self || !buf) return out;

    int r = recv(self->sock, buf, len, 0);
    if (r < 0) {
        out.err = ty_net_last_error();
        return out;
    }

    out.ok = 1;
    out.value = r;
    out.err = 0;
    return out;
}

static void socket_consumer_coro(void* task, void* arg) {
    /* arg: [TySocket*, TyChan*] */
    void** pair = (void**)arg;
    TySocket* sock = (TySocket*)pair[0];
    struct TyChan* chan = (struct TyChan*)pair[1];
    char buf[1024];

    while (1) {
        int r = recv(sock->sock, buf, 1024, 0);
        if (r <= 0) break;
        /* The channel element size is 1 byte (Char/i8).  Send each byte
         * individually so the write matches the slot size.  Passing a
         * char* pointer value into a 1-byte slot would write 8 bytes into
         * a 1-byte region and corrupt adjacent channel memory. */
        for (int i = 0; i < r; i++) {
            ty_chan_send(task, chan, &buf[i]);
        }
    }
    ty_chan_close(chan);
    free(pair);
}

void __ty_method__Socket__consume(void* task, TySocket* self, struct TyChan* chan) {
    void** pair = (void**)malloc(sizeof(void*) * 2);
    pair[0] = self;
    pair[1] = chan;
    ty_spawn(NULL, socket_consumer_coro, pair);
}

TyResult_i32_i32 __ty_method__Socket__write(void* task, TySocket* self, char* buf, int32_t len) {
    (void)task;
    TyResult_i32_i32 out;
    out.ok = 0;
    out.value = 0;
    out.err = -1;

    if (!self || !buf) return out;

    int r = send(self->sock, buf, len, 0);
    if (r < 0) {
        out.err = ty_net_last_error();
        return out;
    }

    out.ok = 1;
    out.value = r;
    out.err = 0;
    return out;
}

void __ty_method__Socket__close(void* task, TySocket* self) {
    (void)task;
    if (!self) return;
    ty_sock_close(self->sock);
    free(self);
}
