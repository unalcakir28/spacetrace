#!/bin/sh
#
# Regenerates crates/scan-core/tests/fixtures/ntfs.tar.gz: real NTFS volume
# images and, for each, what ntfs-3g says about every entry on it.
#
# **Why real images.** The MFT parser in `crates/scan-core/src/ntfs/` is what
# reads the volume on Windows, and none of what can go wrong in it is a type
# error: an unapplied fixup is silently wrong every 512 bytes, a $DATA split
# across extension records loses its size, a DOS name doubles a file. The only
# way to see those without a Windows machine is to parse volumes a real NTFS
# implementation wrote, and to compare entry for entry with what that same
# implementation reports through a mount. That is what the manifests are.
#
# **Why committed.** CI has no privileged Docker, so the tests read the
# images from the tarball and never call this script. Run it again when a case
# is added; the tests take whatever it produced.
#
# Usage: scripts/ntfs-fixtures.sh       (needs Docker; runs privileged, since
#                                        ntfs-3g mounts through FUSE)

set -eu

if [ "${1:-}" != "--inside" ]; then
    repo=$(cd "$(dirname "$0")/.." && pwd)
    exec docker run --rm --privileged --name mftfeat-fixtures \
        -v "$repo:/work" debian:bookworm sh /work/scripts/ntfs-fixtures.sh --inside
fi

apt-get update -qq >/dev/null
apt-get install -y -qq ntfs-3g attr python3 >/dev/null

out=/tmp/fixtures
rm -rf "$out"
mkdir -p "$out"

# File contents. The parser never reads them, so they are text that
# compresses to nothing in the tarball rather than random bytes that do not;
# the one place where incompressible data matters says so.
bytes() {
    yes 'spacetrace ntfs fixture' | head -c "$1"
}

# A REPARSE_DATA_BUFFER as hex, for `setfattr -n system.ntfs_reparse_data`.
# ntfs-3g has no command that makes a Windows symlink or junction, but it
# stores whatever reparse data it is handed, which is all Windows itself
# reads back.
reparse() {
    python3 - "$@" <<'EOF'
import struct, sys
kind, target = sys.argv[1], sys.argv[2]
if kind == "symlink":
    name = target.encode("utf-16-le")
    body = struct.pack("<HHHHI", 0, len(name), len(name), len(name), 1) + name + name
    tag = 0xA000000C
else:
    sub = ("\\??\\" + target).encode("utf-16-le") + b"\0\0"
    prn = target.encode("utf-16-le") + b"\0\0"
    body = struct.pack("<HHHH", 0, len(sub) - 2, len(sub), len(prn) - 2) + sub + prn
    tag = 0xA0000003
print("0x" + (struct.pack("<IHH", tag, len(body), 0) + body).hex())
EOF
}

# What the mount says about every entry, one per line: path, type, size,
# 512-byte blocks, links, mtime, inode (= MFT record). Python's `lstat` rather
# than stat(1), which also asks for extended attributes — ntfs-3g answers that
# with EIO on a reparse point, and the entry would go missing.
manifest() {
    python3 - "$1" >"$2" <<'EOF'
import os, stat, sys
root = sys.argv[1]
rows = []
for d, dirs, files in os.walk(root):
    for n in dirs + files:
        p = os.path.join(d, n)
        s = os.lstat(p)
        kind = "dir" if stat.S_ISDIR(s.st_mode) else "symlink" if stat.S_ISLNK(s.st_mode) else "file"
        rel = os.path.relpath(p, root)
        rows.append(f"{rel}\t{kind}\t{s.st_size}\t{s.st_blocks}\t{s.st_nlink}\t{s.st_mtime_ns // 10**9}\t{s.st_ino}")
print("\n".join(sorted(rows)))
EOF
}

# Fail loudly when a case did not come out as the case it is meant to be, so a
# change in ntfs-3g cannot quietly turn a fixture into a duplicate of another.
require() {
    if ! ntfsinfo -F "$2" "$1" 2>/dev/null | grep -q "$3"; then
        echo "fixture $1: $2 does not have $3" >&2
        exit 1
    fi
}

mount_new() {
    img="$out/$1.img"
    truncate -s "$2" "$img"
    shift 2
    mkntfs -F -q -Q -L fixture "$@" "$img" >/dev/null 2>&1
    mkdir -p /mnt/vol
    ntfs-3g "$img" /mnt/vol
}

# Unmounts, then adds an eighth column: for a directory, the allocated size of
# its $I30 $INDEX_ALLOCATION as ntfsinfo reads it off the image (0 when the
# index fits in $INDEX_ROOT), and `-` for anything else. ntfs-3g's own `stat`
# reports the resident $INDEX_ROOT for a directory, which is not what Windows
# calls a directory's allocation, so the mount cannot be the oracle there.
finish() {
    manifest /mnt/vol "$out/$1.manifest"
    umount /mnt/vol
    python3 - "$out/$1.img" "$out/$1.manifest" <<'EOF'
import re, subprocess, sys
img, path = sys.argv[1], sys.argv[2]
rows = []
for line in open(path, encoding="utf-8").read().splitlines():
    cols = line.split("\t")
    extra = "-"
    if cols[1] == "dir":
        dump = subprocess.run(["ntfsinfo", "-F", "/" + cols[0], img],
                              capture_output=True, text=True, check=True).stdout
        extra = "0"
        part = dump.split("$INDEX_ALLOCATION (0xa0)", 1)
        if len(part) == 2:
            extra = re.search(r"Allocated size:\s+(\d+)", part[1]).group(1)
    rows.append("\t".join(cols + [extra]))
open(path, "w", encoding="utf-8").write("\n".join(rows) + "\n")
EOF
}

# --- features: one of everything, at the default geometry (4 KiB clusters,
# 1 KiB records, so clusters-per-record is stored negative).
mount_new features 24M -c 4096
cd /mnt/vol
mkdir -p nested/a/b/c/d nested/a/sibling emptydir
printf 'hello\n' >nested/a/b/c/d/leaf.txt
printf 'x' >tiny
: >empty
bytes 300 >resident300
bytes 100001 >odd.bin
bytes 300000 >nested/a/sibling/large.bin
# More entries than one index block holds, so the directory gets an
# $INDEX_ALLOCATION and the parser cannot be reading names from the index.
mkdir big
i=0
while [ $i -lt 2500 ]; do : >"big/entry-$i"; i=$((i + 1)); done
printf 'one\n' >big/entry-7
# Hard links, in one directory and across two.
mkdir links1 links2
bytes 5000 >links1/orig
ln links1/orig links2/second
ln links1/orig links1/third
# A sparse file: 4 MiB of hole and one block of data.
bytes 4096 | dd of=sparse.bin bs=4096 count=1 seek=1024 conv=notrunc 2>/dev/null
# A compressed file: the directory carries FILE_ATTRIBUTE_COMPRESSED, so
# ntfs-3g compresses what is created in it.
mkdir comp
setfattr -h -v 0x00000800 -n system.ntfs_attrib_be comp
yes 'compressible line of text' | head -c 200000 >comp/text.txt
head -c 20000 /dev/urandom >comp/random.bin
# An alternate data stream, which is not the file's size on Windows.
printf 'main stream\n' >ads.txt
setfattr -n user.extra -v "$(head -c 3000 /dev/zero | tr '\0' a)" ads.txt
# A long name with characters outside the BMP (UTF-16 surrogate pairs) and a
# Turkish dotless i, and a name with a separate 8.3 DOS name.
long=$(python3 -c 'print("ı" * 100 + "😀" * 40 + ".txt")')
printf 'long\n' >"$long"
printf 'dos\n' >"Long Name With Spaces.txt"
setfattr -n system.ntfs_dos_name -v "LONGNA~1.TXT" "Long Name With Spaces.txt"
# A symlink and a junction, as Windows makes them.
printf 'target\n' >target.txt
: >link.txt
setfattr -n system.ntfs_reparse_data -v "$(reparse symlink target.txt)" link.txt
mkdir junction
setfattr -n system.ntfs_reparse_data -v "$(reparse junction 'C:\target')" junction
# Deleted records: their FILE headers stay, without the in-use flag.
mkdir doomed
i=0
while [ $i -lt 40 ]; do printf '%s\n' $i >"doomed/d$i"; i=$((i + 1)); done
rm -r doomed
i=0
while [ $i -lt 20 ]; do printf 'x\n' >"gone$i"; i=$((i + 1)); done
rm gone*
cd /
finish features
require "$out/features.img" /comp/text.txt "COMPRESSED"
require "$out/features.img" /sparse.bin "SPARSE_FILE"
require "$out/features.img" "/Long Name With Spaces.txt" "DOS"
require "$out/features.img" /ads.txt "extra"
require "$out/features.img" /big '\$INDEX_ALLOCATION'

# --- fragmented: 512-byte clusters (clusters-per-record stored positive).
#
# Two ways a run list outgrows its record. ntfs-3g keeps a growing file
# contiguous wherever it can, so appending in turn is not enough on its own:
#
# * striped.bin writes one cluster and skips one, 600 times. Every hole is a
#   run of its own, the list no longer fits, and ntfs-3g moves it out through
#   a non-resident $ATTRIBUTE_LIST: $DATA in four records, the first carrying
#   the sizes, and the $FILE_NAME in an extension record too.
# * a.bin and b.bin are appended to in turn on a volume that is full but for
#   the holes left by deleting every other spacer, so their runs jump back and
#   forth (negative LCN deltas), and the spacers leave deleted records.
mount_new fragmented 16M -c 512
cd /mnt/vol
i=0
while [ $i -lt 600 ]; do
    bytes 512 | dd of=striped.bin bs=512 count=1 seek=$((i * 2)) conv=notrunc 2>/dev/null
    i=$((i + 1))
done
mkdir frag spacers
ln striped.bin frag/striped-again.bin
bytes 2048 >frag/a.bin
bytes 2048 >frag/b.bin
i=0
while [ $i -lt 900 ]; do bytes 1024 >"spacers/s$i"; i=$((i + 1)); done
head -c 64M /dev/zero >filler 2>/dev/null || true
i=0
while [ $i -lt 900 ]; do rm "spacers/s$i"; i=$((i + 2)); done
i=0
while [ $i -lt 200 ]; do
    bytes 1024 >>frag/a.bin
    bytes 1024 >>frag/b.bin
    i=$((i + 1))
done
printf 'small\n' >frag/small.txt
cd /
finish fragmented
require "$out/fragmented.img" /striped.bin '\$ATTRIBUTE_LIST'
dump=$(ntfsinfo -F /striped.bin "$out/fragmented.img")
base=$(printf '%s\n' "$dump" | sed -n 's/^Dumping Inode \([0-9]*\).*/\1/p')
named=$(printf '%s\n' "$dump" | sed -n 's/.*FILE_NAME (0x30) from mft record \([0-9]*\).*/\1/p' | head -n 1)
if [ "$named" = "$base" ]; then
    echo "fixture fragmented: the \$FILE_NAME of /striped.bin stayed in its base record" >&2
    exit 1
fi
if [ "$(ntfsinfo -v -F /frag/a.bin "$out/fragmented.img" | sed -n 's/^Total runs: \([0-9]*\).*/\1/p')" -lt 10 ]; then
    echo "fixture fragmented: /frag/a.bin is not fragmented" >&2
    exit 1
fi

# --- bigsector: 4 KiB sectors, hence 4 KiB records (eight fixup slots) and
# 64 KiB clusters.
mount_new bigsector 64M -s 4096 -c 65536
cd /mnt/vol
mkdir -p d1/d2
bytes 200000 >d1/two-clusters-and-more.bin
printf 'r\n' >d1/d2/resident.txt
ln d1/d2/resident.txt d1/resident-again.txt
i=0
while [ $i -lt 300 ]; do : >"d1/e$i"; i=$((i + 1)); done
cd /
finish bigsector

cd "$out"
tar -czf /work/crates/scan-core/tests/fixtures/ntfs.tar.gz ./*.img ./*.manifest
ls -l "$out" /work/crates/scan-core/tests/fixtures/ntfs.tar.gz
