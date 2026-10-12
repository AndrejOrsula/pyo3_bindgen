# Browser VM build provenance

`prepare-vm.py` builds the serial-only guest assets used by the browser compiler.
It reads source and configuration pins from `assets.json`, verifies every input
by SHA-256 and size, and writes `seabios-built.bin`, `bzImage`,
`initramfs.cpio.gz`, and `vm-build.json` to an empty output directory.
Missing source and tool archives are fetched from their pinned URLs into the
cache.

From the directory containing this document and `prepare-vm.py`, run:

```sh
python3 prepare-vm.py \
  --cache-dir /path/to/pinned-source-cache \
  --output-dir /path/to/empty-output
```

The source archive URLs, sizes, and hashes are owned by `assets.json`. The VM
inputs are Linux 6.8.12 from kernel.org, BusyBox 1.36.1 from busybox.net, and
the official SeaBIOS 1.16.2 source release. Its `rel-1.16.2` tag resolves to
QEMU googlesource commit `ea1b7a0733906b8425d948ae94fba63c32b1d425`. The
matching Linux, BusyBox, and SeaBIOS configs and the guest `/init` script are local pins in
`provenance/`. The source archives retain these license notices:

| Source | Notice member | SHA-256 | Bytes |
|---|---|---|---:|
| Linux | `COPYING` | `fb5a425bd3b3cd6071a3a9aff9909a859e7c1158d54d32e07658398cd67eb6a0` | 496 |
| Linux | `LICENSES/preferred/GPL-2.0` | `f6b78c087c3ebdf0f3c13415070dd480a3f35d8fc76f3d02180a407c1c812f79` | 18,729 |
| BusyBox | `LICENSE` | `bbfc9843646d483c334664f651c208b9839626891d8f17604db2146962f43548` | 18,348 |
| SeaBIOS | `COPYING` | `8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903` | 35,147 |
| SeaBIOS | `COPYING.LESSER` | `a853c2ffec17057872340eee242ae4d96cbf2b520ae27d903e1b2fef1a5f9d1c` | 7,639 |

Linux and BusyBox are GPL-2.0; SeaBIOS is LGPL-3.0-or-later. Distribute the
source archives and notices alongside generated guest binaries as required by
their licenses. Build-only host tools are not copied to the guest or output
directory.

The supported clean-build host is Ubuntu 22.04 x86_64 with GCC 11 or newer,
GNU binutils `ld` 2.38 or newer, GNU Make 4.3 or newer, Python 3, and the
kernel build utilities listed by the script. The script fetches and verifies
task-local build tools: Zig 0.13.0, GNU Bison 3.8.2, Flex 2.6.4, GNU m4 1.4.18,
ACPICA 20200925-6, and libsigsegv2 2.13-1ubuntu3. Each exact URL, version,
archive size, and SHA-256 is recorded in `vm-build.json`; Debian package
copyright metadata remains in the cache. No system packages are installed.

The BusyBox compiler wrapper `provenance/zig-cc` invokes the pinned Zig compiler
for `x86-linux-musl`. The recipe builds Linux with `ARCH=i386`, builds static
BusyBox with the same i386 target, then installs the applets into the initramfs.
It checks that the installed BusyBox is byte-identical to the build output and
is a static ELF32 Intel 80386 executable. The init script mounts devtmpfs,
proc, sysfs, an executable tmpfs at `/tmp`, and the v86 9p host share at `/mnt`;
it emits `BINDGEN_VM_READY` after the 9p mount succeeds and then starts the
interactive BusyBox shell.

The pinned SeaBIOS release archive has a `seabios-1.16.2/` root directory and
adds `.version` containing `1.16.2`. Its 293 tracked source files match the
pinned release commit by content. Tracked-file modes differ only by group-write
bits. The recipe unpacks that root directory and overwrites `.version`
with `rel-1.16.2-0-gea1b7a0`, the version recorded by the tag, before building.
It also uses the fixed `EXTRAVERSION=-pyo3-browser-v1`. This uses SeaBIOS's
upstream version generator in its clean-build mode and avoids embedding a
wall-clock timestamp or build host name; the script rejects either if they
appear in the output. The release tarball is used because the Gitiles archive
endpoint rewrites member mtimes between requests and cannot serve as a stable
byte-pinned build input.

Linux build commands run with a fixed UTC build timestamp, user, host, version,
`SOURCE_DATE_EPOCH`, and `KCONFIG_NOTIMESTAMP`. The initramfs writer orders
entries by encoded path, fixes UID/GID and mtimes, writes `newc`, then applies
gzip level 9 with a zero timestamp. These controls make packaging deterministic
and make outputs reproducible on the documented, matching toolchain. They do
not promise identical kernel or firmware bytes across arbitrary compiler,
binutils, or host versions; `vm-build.json` records observed host versions and
the exact output hashes for each run.

Build subprocesses preserve the caller's `PATH`, then prepend the task-local
tool directory; they retain task-local Zig cache paths. They discard inherited
`MAKEFLAGS`, `MFLAGS`, `MAKEOVERRIDES`, `GNUMAKEFLAGS`, `MAKEFILES`, `MAKELEVEL`,
and `MAKE`, along with compiler selection and flags (`CC`, `HOSTCC`,
`CROSS_COMPILE`, `CFLAGS`, `KCFLAGS`, `CFLAGS_KERNEL`, `HOSTLDFLAGS`, and related
tool variables), all inherited `KBUILD_*`, `KCONFIG_*`, and `CONFIG_*` variables,
compiler include/library search overrides, tool selectors (`BISON`, `FLEX`,
`YACC`, `PERL`, `PYTHON`), Python/Perl import overrides, and archive-tool option
variables. The task-local `LD_LIBRARY_PATH` contains only the pinned build-tool
library directory. `HOME` is left unchanged. Explicit command-line assignments
select the BusyBox target compiler and Linux/BusyBox architecture.

The recipe's full upstream builds are `make olddefconfig && make EXTRAVERSION=-pyo3-browser-v1 -jN`
for SeaBIOS, `make ARCH=i386 olddefconfig && make -s ARCH=i386 -jN bzImage` for Linux,
and `make ARCH=i386 oldconfig && make ARCH=i386 CC=provenance/zig-cc -jN` plus
`make ARCH=i386 CC=provenance/zig-cc CONFIG_PREFIX=<rootfs> install` for BusyBox.
`N` is bounded by the script to 16 workers. The generated receipt contains the
effective config hashes and actual byte sizes and SHA-256 values; it uses
relative output filenames and contains no scratch paths or host-specific paths.
