#!/usr/bin/env python3
"""ohc-nfsd — a tiny read-only NFSv3 server, for netbooting a Control4 box.

Why this exists: netbooting the EA1 needs an NFS root, and the OS NFS servers all
want root (they bind the privileged ports 111/2049 and edit /etc/exports). This
serves a directory read-only from **unprivileged ports** instead, so no sudo is
involved anywhere. The client skips the portmapper by being told the ports
explicitly:

    mount -t nfs -o ro,nolock,vers=3,tcp,port=2049,mountport=2050 host:/ea1 /mnt

Read-only on purpose. A netbooted root that cannot write is exactly what you want
while proving a boot path, and it keeps the whole implementation to the handful
of NFSv3 calls Linux issues when mounting a root filesystem.

    ohc-nfsd.py --root ./nfsroot --nfs-port 2049 --mount-port 2050

DO NOT confuse this with Control4's `/dev_prog_boot`, which also mounts NFS but
then FORMATS the eMMC and reflashes the SPI-NOR. See docs/boot-chain.md.
"""

import argparse
import os
import socket
import socketserver
import stat
import struct
import sys
import threading

# ── RPC / program constants ──────────────────────────────────────────────────
PROG_MOUNT, VERS_MOUNT = 100005, 3
PROG_NFS, VERS_NFS = 100003, 3

MSG_CALL, MSG_REPLY = 0, 1
REPLY_ACCEPTED = 0
ACCEPT_SUCCESS, ACCEPT_PROG_UNAVAIL, ACCEPT_PROG_MISMATCH, ACCEPT_PROC_UNAVAIL = 0, 1, 2, 3

NFS3_OK, NFS3ERR_NOENT, NFS3ERR_IO, NFS3ERR_ACCES = 0, 2, 5, 13
NFS3ERR_NOTDIR, NFS3ERR_INVAL, NFS3ERR_ROFS, NFS3ERR_STALE = 20, 22, 30, 70

NF3REG, NF3DIR, NF3LNK = 1, 2, 5


# ── XDR helpers ──────────────────────────────────────────────────────────────
def u32(v):
    return struct.pack(">I", v)


def u64(v):
    return struct.pack(">Q", v)


def opaque(data):
    """Variable-length opaque: length then 4-byte-aligned payload."""
    pad = (-len(data)) % 4
    return u32(len(data)) + data + b"\0" * pad


class Reader:
    def __init__(self, buf):
        self.b, self.i = buf, 0

    def u32(self):
        v = struct.unpack_from(">I", self.b, self.i)[0]
        self.i += 4
        return v

    def u64(self):
        v = struct.unpack_from(">Q", self.b, self.i)[0]
        self.i += 8
        return v

    def opaque(self):
        n = self.u32()
        v = self.b[self.i : self.i + n]
        self.i += n + ((-n) % 4)
        return v

    def fixed(self, n):
        """Fixed-length opaque: NO length prefix (e.g. cookieverf3[8])."""
        v = self.b[self.i : self.i + n]
        self.i += n + ((-n) % 4)
        return v

    def string(self):
        return self.opaque().decode("utf-8", "replace")


# ── File handle table ────────────────────────────────────────────────────────
class Handles:
    """Maps opaque handles <-> paths.

    Handles must stay valid for the life of the mount, and the client may hand
    one back at any time, so this only ever grows. That is fine for a boot-time
    root of a few thousand files.
    """

    def __init__(self, root):
        self.root = os.path.realpath(root)
        self._to_path = {}
        self._to_handle = {}
        self.get(self.root)

    def get(self, path):
        # abspath, NOT realpath: realpath resolves symlinks, which erases the
        # link's own identity. The client then gets the target's attributes,
        # READLINK reports "not a link", and the dynamic loader cannot follow
        # /lib/ld-linux.so.2 -> ld-2.19.so. Containment is still checked with
        # realpath in contains(), so this does not weaken the export boundary.
        path = os.path.abspath(path)
        if path in self._to_handle:
            return self._to_handle[path]
        h = struct.pack(">I", len(self._to_path) + 1) + b"\0" * 28
        self._to_handle[path] = h
        self._to_path[h] = path
        return h

    def path(self, h):
        return self._to_path.get(bytes(h))

    def contains(self, path):
        """Refuse anything that escaped the export root (symlink games)."""
        rp = os.path.realpath(path)
        return rp == self.root or rp.startswith(self.root + os.sep)


def fattr3(st):
    if stat.S_ISDIR(st.st_mode):
        ftype = NF3DIR
    elif stat.S_ISLNK(st.st_mode):
        ftype = NF3LNK
    else:
        ftype = NF3REG
    return b"".join([
        u32(ftype),
        u32(stat.S_IMODE(st.st_mode)),
        u32(st.st_nlink),
        u32(0),                      # uid: everything maps to root
        u32(0),                      # gid
        u64(st.st_size),
        u64(st.st_blocks * 512),
        u32(0), u32(0),              # rdev
        u64(st.st_dev & 0xFFFFFFFFFFFFFFFF),
        u64(st.st_ino & 0xFFFFFFFFFFFFFFFF),
        u32(int(st.st_atime)), u32(0),
        u32(int(st.st_mtime)), u32(0),
        u32(int(st.st_ctime)), u32(0),
    ])


def post_op_attr(path):
    try:
        return u32(1) + fattr3(os.lstat(path))
    except OSError:
        return u32(0)


# ── The server ───────────────────────────────────────────────────────────────
class Nfs:
    def __init__(self, root, verbose=False):
        self.handles = Handles(root)
        self.root = self.handles.root
        self.verbose = verbose

    def log(self, *a):
        if self.verbose:
            print("   ", *a, flush=True)

    # -- MOUNT program --------------------------------------------------------
    def mount_call(self, proc, r):
        if proc == 0:                                    # NULL
            return b""
        if proc == 1:                                    # MNT
            path = r.string()
            # Honour the requested subdirectory: "host:/ea1" must land on
            # <export>/ea1, not on the export root. Serving the root regardless
            # silently gives the client the wrong tree.
            target = os.path.join(self.root, path.lstrip("/")) if path.strip("/") else self.root
            ok = os.path.isdir(target) and self.handles.contains(target)
            print(f">> MNT {path!r} -> {target}{'' if ok else '  [REJECTED]'}", flush=True)
            if not ok:
                return u32(NFS3ERR_NOENT)
            h = self.handles.get(target)
            return u32(0) + opaque(h) + u32(1) + u32(0)  # ok, fh, 1 auth flavour (AUTH_NULL)
        if proc == 3:                                    # UMNT
            r.string()
            print(">> UMNT", flush=True)
            return b""
        if proc == 5:                                    # EXPORT
            return u32(1) + opaque(b"/") + u32(0) + u32(0)
        return None

    # -- NFS program ----------------------------------------------------------
    def nfs_call(self, proc, r):
        if proc == 0:                                    # NULL
            return b""

        if proc == 1:                                    # GETATTR
            p = self.handles.path(r.opaque())
            if not p or not os.path.lexists(p):
                return u32(NFS3ERR_STALE)
            return u32(NFS3_OK) + fattr3(os.lstat(p))

        if proc == 3:                                    # LOOKUP
            dirp = self.handles.path(r.opaque())
            name = r.string()
            if not dirp:
                return u32(NFS3ERR_STALE) + u32(0)
            target = os.path.join(dirp, name)
            if not os.path.lexists(target) or not self.handles.contains(target):
                return u32(NFS3ERR_NOENT) + post_op_attr(dirp)
            self.log("LOOKUP", name)
            return (u32(NFS3_OK) + opaque(self.handles.get(target))
                    + post_op_attr(target) + post_op_attr(dirp))

        if proc == 4:                                    # ACCESS
            p = self.handles.path(r.opaque())
            want = r.u32()
            if not p:
                return u32(NFS3ERR_STALE) + u32(0)
            # ACCESS3: READ 0x01, LOOKUP 0x02, MODIFY 0x04, EXTEND 0x08,
            # DELETE 0x10, EXECUTE 0x20. Grant read+lookup+execute and drop the
            # write bits. Masking LOOKUP out by mistake makes every directory
            # traversal fail as "Permission denied", which is a confusing way to
            # discover a one-character error.
            granted = want & 0x23
            return u32(NFS3_OK) + post_op_attr(p) + u32(granted)

        if proc == 5:                                    # READLINK
            p = self.handles.path(r.opaque())
            if not p or not os.path.islink(p):
                return u32(NFS3ERR_INVAL) + u32(0)
            return (u32(NFS3_OK) + post_op_attr(p)
                    + opaque(os.readlink(p).encode()))

        if proc == 6:                                    # READ
            p = self.handles.path(r.opaque())
            off, count = r.u64(), r.u32()
            if not p:
                return u32(NFS3ERR_STALE) + u32(0)
            try:
                with open(p, "rb") as fh:
                    fh.seek(off)
                    data = fh.read(count)
            except OSError:
                return u32(NFS3ERR_IO) + post_op_attr(p)
            eof = 1 if off + len(data) >= os.path.getsize(p) else 0
            return (u32(NFS3_OK) + post_op_attr(p) + u32(len(data))
                    + u32(eof) + opaque(data))

        if proc in (16, 17):                             # READDIR / READDIRPLUS
            p = self.handles.path(r.opaque())
            cookie = r.u64()
            r.fixed(8)                                   # cookieverf3 is FIXED 8B
            if proc == 17:
                r.u32()                                  # dircount
            r.u32()                                      # maxcount
            if not p or not os.path.isdir(p):
                return u32(NFS3ERR_NOTDIR) + u32(0)

            names = [".", ".."] + sorted(os.listdir(p))
            # cookieverf3 is a fixed 8-byte opaque — emitting it with a length
            # prefix shifts the whole entry list and the client sees an empty
            # directory with no error at all.
            out = [u32(NFS3_OK), post_op_attr(p), b"\0" * 8]
            # `cookie` is an index into that list; 0 means start from the top.
            emitted = 0
            for idx in range(int(cookie), len(names)):
                name = names[idx]
                target = os.path.join(p, name) if name not in (".", "..") else (
                    p if name == "." else os.path.dirname(p))
                try:
                    st = os.lstat(target)
                except OSError:
                    continue
                out.append(u32(1))                       # value follows
                out.append(u64(st.st_ino & 0xFFFFFFFFFFFFFFFF))
                out.append(opaque(name.encode()))
                out.append(u64(idx + 1))                 # next cookie
                if proc == 17:
                    out.append(post_op_attr(target))
                    out.append(u32(1) + opaque(self.handles.get(target)))
                emitted += 1
                # Keep replies well inside the client's buffer.
                if emitted >= 64:
                    out.append(u32(0) + u32(0))          # no more, not eof
                    return b"".join(out)
            out.append(u32(0))                           # end of list
            out.append(u32(1))                           # eof
            return b"".join(out)

        if proc == 18:                                   # FSSTAT
            p = self.handles.path(r.opaque()) or self.root
            vfs = os.statvfs(p)
            total = vfs.f_blocks * vfs.f_frsize
            free = vfs.f_bavail * vfs.f_frsize
            return (u32(NFS3_OK) + post_op_attr(p)
                    + u64(total) + u64(free) + u64(free)
                    + u64(vfs.f_files) + u64(vfs.f_ffree) + u64(vfs.f_ffree)
                    + u32(0))

        if proc == 19:                                   # FSINFO
            p = self.handles.path(r.opaque()) or self.root
            mx = 65536
            return (u32(NFS3_OK) + post_op_attr(p)
                    + u32(mx) + u32(mx) + u32(4096)      # rtmax/rtpref/rtmult
                    + u32(mx) + u32(mx) + u32(4096)      # wtmax/wtpref/wtmult
                    + u32(4096)                          # dtpref
                    + u64(0xFFFFFFFFFFFFFFFF)            # maxfilesize
                    + u32(1) + u32(0)                    # time_delta
                    + u32(0x1B))                         # properties

        if proc == 20:                                   # PATHCONF
            p = self.handles.path(r.opaque()) or self.root
            return (u32(NFS3_OK) + post_op_attr(p)
                    + u32(255) + u32(255) + u32(0) + u32(1) + u32(1) + u32(1))

        # Every mutating call: this export is read-only, say so honestly rather
        # than failing in some confusing way.
        if proc in (2, 7, 8, 9, 10, 11, 12, 13, 14, 15, 21):
            return u32(NFS3ERR_ROFS) + u32(0) + u32(0)
        return None


def handle_rpc(nfs, prog_expected, body):
    """Parse one RPC call and return the reply body (without record marking)."""
    r = Reader(body)
    xid = r.u32()
    if r.u32() != MSG_CALL:
        return None
    rpcvers, prog, vers, proc = r.u32(), r.u32(), r.u32(), r.u32()
    # opaque_auth is { u32 flavour; opaque body<400> } — in that order. Reading
    # it the other way round happens to consume the same 8 bytes for AUTH_NULL,
    # but Linux mounts with AUTH_SYS, whose non-empty body then shifts every
    # following argument and turns file handles into garbage (STALE).
    r.u32(); r.opaque()      # cred
    r.u32(); r.opaque()      # verf

    def reply(accept_stat, payload=b"", mismatch=b""):
        return (u32(xid) + u32(MSG_REPLY) + u32(REPLY_ACCEPTED)
                + u32(0) + opaque(b"")           # verf AUTH_NULL
                + u32(accept_stat) + mismatch + payload)

    if rpcvers != 2:
        return reply(ACCEPT_PROG_MISMATCH, mismatch=u32(2) + u32(2))
    if prog != prog_expected:
        return reply(ACCEPT_PROG_UNAVAIL)

    want_vers = VERS_MOUNT if prog == PROG_MOUNT else VERS_NFS
    if vers != want_vers:
        return reply(ACCEPT_PROG_MISMATCH, mismatch=u32(want_vers) + u32(want_vers))

    out = (nfs.mount_call(proc, r) if prog == PROG_MOUNT else nfs.nfs_call(proc, r))
    if out is None:
        return reply(ACCEPT_PROC_UNAVAIL)
    return reply(ACCEPT_SUCCESS, out)


class RpcHandler(socketserver.BaseRequestHandler):
    prog = None
    nfs = None

    def handle(self):
        sock = self.request
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        buf = b""
        while True:
            try:
                chunk = sock.recv(65536)
            except OSError:
                return
            if not chunk:
                return
            buf += chunk
            # TCP record marking: u32 with the high bit set on the last fragment.
            while len(buf) >= 4:
                mark = struct.unpack(">I", buf[:4])[0]
                length = mark & 0x7FFFFFFF
                if len(buf) < 4 + length:
                    break
                body, buf = buf[4 : 4 + length], buf[4 + length :]
                try:
                    out = handle_rpc(self.nfs, self.prog, body)
                except Exception as e:                      # never kill the mount
                    print(f"!! rpc error: {e}", flush=True)
                    out = None
                if out:
                    sock.sendall(u32(0x80000000 | len(out)) + out)


class Threaded(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    ap = argparse.ArgumentParser(description="read-only NFSv3 server, no root needed")
    ap.add_argument("--root", required=True, help="directory to export")
    ap.add_argument("--bind", default="0.0.0.0")
    ap.add_argument("--nfs-port", type=int, default=2049)
    ap.add_argument("--mount-port", type=int, default=2050)
    ap.add_argument("-v", "--verbose", action="store_true")
    a = ap.parse_args()

    if not os.path.isdir(a.root):
        sys.exit(f"not a directory: {a.root}")
    nfs = Nfs(a.root, a.verbose)

    servers = []
    for port, prog, label in ((a.mount_port, PROG_MOUNT, "mountd"),
                              (a.nfs_port, PROG_NFS, "nfsd")):
        handler = type(f"H{label}", (RpcHandler,), {"prog": prog, "nfs": nfs})
        srv = Threaded((a.bind, port), handler)
        servers.append(srv)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        print(f"{label:7} on {a.bind}:{port}", flush=True)

    print(f"exporting {nfs.root} read-only\nmount with:\n"
          f"  mount -t nfs -o ro,nolock,vers=3,tcp,"
          f"port={a.nfs_port},mountport={a.mount_port} <host>:/ea1 /mnt", flush=True)
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        for s in servers:
            s.shutdown()


if __name__ == "__main__":
    main()
